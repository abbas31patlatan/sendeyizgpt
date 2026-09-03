import { FormEvent, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import {
  createConversation,
  getRuntimeAvailability,
  getRuntimeSnapshot,
  getRuntimeStatus,
  listConversations,
  listMessages,
  loadLocalModel,
  onGenerationEvent,
  startGeneration,
  stopEverything,
  unloadLocalModel,
} from "./ipc";
import {
  formatBytes,
  initialRuntimeSnapshot,
  initialUnavailableStatus,
  type Conversation,
  type LoadPreset,
  type Message,
  type RuntimeAvailability,
  type RuntimeSnapshot,
  type RuntimeStatus,
} from "./protocol";

type View = "chats" | "workspaces" | "models" | "automations";

const navigation: Array<{ id: View; label: string; glyph: string }> = [
  { id: "chats", label: "Chats", glyph: "◌" },
  { id: "workspaces", label: "Workspaces", glyph: "⌘" },
  { id: "models", label: "Models", glyph: "▣" },
  { id: "automations", label: "Automations", glyph: "◷" },
];

const presetDetails: Record<LoadPreset, { context: number; offload: number; description: string }> = {
  eco: { context: 4096, offload: 75, description: "Lower VRAM pressure and quieter operation" },
  balanced: { context: 8192, offload: 100, description: "Recommended performance and efficiency balance" },
  performance: { context: 16384, offload: 100, description: "Higher throughput with more memory use" },
};

function errorText(error: unknown): string {
  if (error instanceof Error) return error.message;
  if (typeof error === "string") return error;
  return "The operation failed unexpectedly.";
}

function App() {
  const [view, setView] = useState<View>("chats");
  const [runtime, setRuntime] = useState<RuntimeStatus>(initialUnavailableStatus);
  const [snapshot, setSnapshot] = useState<RuntimeSnapshot>(initialRuntimeSnapshot);
  const [availability, setAvailability] = useState<RuntimeAvailability | null>(null);
  const [conversations, setConversations] = useState<Conversation[]>([]);
  const [activeConversation, setActiveConversation] = useState<string | null>(null);
  const [messages, setMessages] = useState<Message[]>([]);
  const [draft, setDraft] = useState("");
  const [streamingText, setStreamingText] = useState("");
  const [modelPath, setModelPath] = useState("");
  const [preset, setPreset] = useState<LoadPreset>("balanced");
  const [contextLength, setContextLength] = useState(8192);
  const [gpuOffload, setGpuOffload] = useState(100);
  const [cpuThreads, setCpuThreads] = useState(Math.max(2, navigator.hardwareConcurrency || 8));
  const [busy, setBusy] = useState<"loading-model" | "generating" | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const activeOperation = useRef<string | null>(null);
  const activeConversationRef = useRef<string | null>(null);

  useEffect(() => {
    activeConversationRef.current = activeConversation;
  }, [activeConversation]);

  const refreshRuntime = useCallback(async () => {
    const [status, current, bundled] = await Promise.all([
      getRuntimeStatus(),
      getRuntimeSnapshot(),
      getRuntimeAvailability(),
    ]);
    setRuntime(status);
    setSnapshot(current);
    setAvailability(bundled);
  }, []);

  const refreshConversations = useCallback(async () => {
    const items = await listConversations();
    setConversations(items);
    return items;
  }, []);

  useEffect(() => {
    void Promise.all([refreshRuntime(), refreshConversations()]).catch((error) => {
      setNotice(errorText(error));
    });
  }, [refreshConversations, refreshRuntime]);

  useEffect(() => {
    let disposed = false;
    let cleanup: (() => void) | undefined;
    void onGenerationEvent((event) => {
      if (event.operation_id !== activeOperation.current) return;
      if (event.type === "delta") {
        setStreamingText((current) => current + event.text);
      } else if (event.type === "finished") {
        setBusy(null);
        setStreamingText("");
        activeOperation.current = null;
        const conversationId = activeConversationRef.current;
        if (conversationId) {
          void listMessages(conversationId).then(setMessages).catch((error) => setNotice(errorText(error)));
        }
        void Promise.all([refreshRuntime(), refreshConversations()]);
      } else if (event.type === "failed") {
        setBusy(null);
        setStreamingText("");
        activeOperation.current = null;
        setNotice(event.message);
      }
    }).then((unlisten) => {
      if (disposed) unlisten();
      else cleanup = unlisten;
    });
    return () => {
      disposed = true;
      cleanup?.();
    };
  }, [refreshConversations, refreshRuntime]);

  useEffect(() => {
    let cleanup: (() => void) | undefined;
    void getCurrentWindow().onDragDropEvent((event) => {
      if (event.payload.type !== "drop") return;
      const gguf = event.payload.paths.find((path) => path.toLowerCase().endsWith(".gguf"));
      if (gguf) {
        setModelPath(gguf);
        setView("models");
        setNotice("GGUF model selected. Review the profile and choose Load model.");
      }
    }).then((unlisten) => {
      cleanup = unlisten;
    });
    return () => cleanup?.();
  }, []);

  const selectConversation = async (id: string) => {
    setNotice(null);
    setActiveConversation(id);
    setView("chats");
    try {
      setMessages(await listMessages(id));
    } catch (error) {
      setNotice(errorText(error));
    }
  };

  const newConversation = () => {
    setActiveConversation(null);
    setMessages([]);
    setStreamingText("");
    setView("chats");
    setNotice(null);
  };

  const handleSubmit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const content = draft.trim();
    if (!content || busy || !snapshot.running) return;
    setNotice(null);
    setDraft("");
    setStreamingText("");
    setBusy("generating");
    try {
      let conversationId = activeConversation;
      if (!conversationId) {
        const conversation = await createConversation(content);
        conversationId = conversation.id;
        setActiveConversation(conversation.id);
        activeConversationRef.current = conversation.id;
        setConversations((current) => [conversation, ...current]);
      }
      const optimistic: Message = {
        id: crypto.randomUUID(),
        conversation_id: conversationId,
        role: "user",
        content,
        created_at: new Date().toISOString(),
      };
      const outbound = [...messages, optimistic]
        .filter((message) => message.role === "user" || message.role === "assistant")
        .map(({ role, content: messageContent }) => ({ role, content: messageContent }));
      setMessages((current) => [...current, optimistic]);
      const operationId = await startGeneration({ conversationId, messages: outbound });
      activeOperation.current = operationId;
    } catch (error) {
      setBusy(null);
      setNotice(errorText(error));
    }
  };

  const handleLoadModel = async () => {
    if (!modelPath.trim()) {
      setNotice("Drag a .gguf file onto the window or paste its full path first.");
      return;
    }
    setNotice(null);
    setBusy("loading-model");
    try {
      const loaded = await loadLocalModel({
        modelPath: modelPath.trim(),
        preset,
        contextLength,
        cpuThreads,
        gpuOffloadPercent: gpuOffload,
      });
      setSnapshot(loaded);
      await refreshRuntime();
      setView("chats");
      setNotice(`${loaded.model_name ?? "Model"} is ready.`);
    } catch (error) {
      setNotice(errorText(error));
    } finally {
      setBusy(null);
    }
  };

  const handleUnloadModel = async () => {
    setNotice(null);
    try {
      await unloadLocalModel();
      await refreshRuntime();
    } catch (error) {
      setNotice(errorText(error));
    }
  };

  const handleStop = async () => {
    try {
      const count = await stopEverything();
      activeOperation.current = null;
      setBusy(null);
      setStreamingText("");
      await refreshRuntime();
      setNotice(count === 0 ? "Model stopped." : `${count} active operation(s) stopped.`);
    } catch (error) {
      setNotice(errorText(error));
    }
  };

  const applyPreset = (next: LoadPreset) => {
    setPreset(next);
    setContextLength(presetDetails[next].context);
    setGpuOffload(presetDetails[next].offload);
  };

  const viewTitle = useMemo(
    () => navigation.find((item) => item.id === view)?.label ?? "SendeyizGPT",
    [view],
  );

  return (
    <div className="app-shell">
      <header className="topbar">
        <div className="brand-lockup">
          <div className="brand-mark" aria-hidden="true">S</div>
          <div><div className="brand-name">SendeyizGPT</div><div className="brand-subtitle">local AI environment</div></div>
        </div>
        <div className="topbar-status">
          <span className={`status-dot ${snapshot.running ? "is-ready" : ""}`} />
          <span>{snapshot.running ? "Model ready" : "Model offline"}</span>
          <span className="topbar-divider" />
          <span className="muted">v{runtime.app_version}</span>
        </div>
      </header>

      <div className="workspace-grid">
        <aside className="sidebar panel-divider-right">
          <div className="sidebar-actions">
            <button className="new-chat-button" type="button" onClick={newConversation}>
              <span aria-hidden="true">+</span><span>New chat</span><kbd>Ctrl K</kbd>
            </button>
          </div>
          <nav className="primary-nav" aria-label="Primary navigation">
            {navigation.map((item) => (
              <button className={`nav-item ${view === item.id ? "is-active" : ""}`} key={item.id} type="button" onClick={() => setView(item.id)}>
                <span className="nav-glyph" aria-hidden="true">{item.glyph}</span><span>{item.label}</span>
              </button>
            ))}
          </nav>
          <div className="sidebar-section conversation-list">
            <div className="section-label">Recent chats</div>
            {conversations.length === 0 ? <div className="empty-sidebar-state">No conversations yet</div> : conversations.map((conversation) => (
              <button className={`conversation-item ${activeConversation === conversation.id ? "is-active" : ""}`} key={conversation.id} type="button" onClick={() => void selectConversation(conversation.id)} title={conversation.title}>
                {conversation.title}
              </button>
            ))}
          </div>
          <div className="sidebar-section sidebar-bottom">
            <div className="privacy-note"><span className="status-dot is-ready" /> Local data only</div>
          </div>
        </aside>

        <main className="main-panel">
          <div className="content-toolbar">
            <div><div className="eyebrow">Workspace</div><h1>{viewTitle}</h1></div>
            {view === "models" && snapshot.running && <button className="text-button danger" type="button" onClick={() => void handleUnloadModel()}>Unload model</button>}
          </div>
          {notice && <div className="notice" role="status"><span>{notice}</span><button type="button" onClick={() => setNotice(null)}>×</button></div>}

          {view === "chats" && (
            <section className="chat-surface">
              {messages.length === 0 && !streamingText ? (
                <div className="welcome-block compact-welcome">
                  <div className="welcome-orbit" aria-hidden="true"><span className="orbit-dot orbit-dot-one" /><span className="orbit-dot orbit-dot-two" /><div className="orbit-core">S</div></div>
                  <div className="eyebrow">Local, isolated, controlled</div>
                  <h2>{snapshot.running ? "What are you working on?" : "Load your first GGUF model"}</h2>
                  <p>{snapshot.running ? "Your conversation is stored locally in SQLite and generated by the isolated llama.cpp worker." : "Open Models, drag a GGUF file onto this window, choose a profile and load it. AMD Vulkan is included in the Windows package."}</p>
                  {!snapshot.running && <button className="primary-button welcome-action" type="button" onClick={() => setView("models")}>Open model manager</button>}
                </div>
              ) : (
                <div className="message-list">
                  {messages.map((message) => (
                    <article className={`message message-${message.role}`} key={message.id}>
                      <div className="message-role">{message.role === "user" ? "You" : message.role === "assistant" ? "SendeyizGPT" : message.role}</div>
                      <div className="message-content">{message.content}</div>
                    </article>
                  ))}
                  {busy === "generating" && <article className="message message-assistant streaming"><div className="message-role">SendeyizGPT <span className="typing-dot" /></div><div className="message-content">{streamingText || "Thinking…"}</div></article>}
                </div>
              )}
              <form className="composer-wrap sticky-composer" onSubmit={(event) => void handleSubmit(event)}>
                <textarea aria-label="Message composer" value={draft} onChange={(event) => setDraft(event.target.value)} onKeyDown={(event) => {
                  if (event.key === "Enter" && !event.shiftKey) {
                    event.preventDefault();
                    event.currentTarget.form?.requestSubmit();
                  }
                }} placeholder={snapshot.running ? "Message your local model…" : "Load a model to start chatting…"} rows={3} disabled={!snapshot.running || busy !== null} />
                <div className="composer-footer"><div className="composer-hint"><span className="lock-icon" aria-hidden="true">⌑</span>{snapshot.running ? `${snapshot.model_name} · ${snapshot.accelerator}` : "No model loaded"}</div><button className="send-button" type="submit" disabled={!draft.trim() || !snapshot.running || busy !== null} aria-label="Send message">↑</button></div>
              </form>
            </section>
          )}

          {view === "models" && (
            <section className="model-manager">
              <div className="model-hero"><div><div className="eyebrow">Bundled runtime</div><h2>llama.cpp Vulkan</h2><p>Runs as an isolated local worker. Network binding is restricted to 127.0.0.1 and protected by an ephemeral API key.</p></div><span className={`pill ${availability?.available ? "pill-green" : "pill-warn"}`}>{availability?.available ? "AVAILABLE" : "MISSING"}</span></div>
              <div className="model-form card">
                <label className="field"><span>GGUF model path</span><input value={modelPath} onChange={(event) => setModelPath(event.target.value)} placeholder="C:\Models\Qwen3-8B-Q4_K_M.gguf" /></label>
                <div className="drop-hint">Tip: drag and drop a .gguf file anywhere in this window.</div>
                <div className="preset-grid">{(["eco", "balanced", "performance"] as const).map((item) => <button className={`preset-card ${preset === item ? "is-active" : ""}`} type="button" key={item} onClick={() => applyPreset(item)}><strong>{item[0].toUpperCase() + item.slice(1)}</strong><small>{presetDetails[item].description}</small></button>)}</div>
                <div className="advanced-grid">
                  <label className="field"><span>Context length</span><input type="number" min={512} max={131072} step={512} value={contextLength} onChange={(event) => setContextLength(Number(event.target.value))} /></label>
                  <label className="field"><span>GPU offload %</span><input type="number" min={0} max={100} value={gpuOffload} onChange={(event) => setGpuOffload(Number(event.target.value))} /></label>
                  <label className="field"><span>CPU threads</span><input type="number" min={1} max={256} value={cpuThreads} onChange={(event) => setCpuThreads(Number(event.target.value))} /></label>
                </div>
                <div className="load-summary"><span>Backend: Vulkan</span><span>Context: {contextLength.toLocaleString()}</span><span>GPU offload: {gpuOffload}%</span></div>
                <button className="primary-button load-button" type="button" disabled={busy !== null || !availability?.available} onClick={() => void handleLoadModel()}>{busy === "loading-model" ? "Loading model…" : "Load model"}</button>
              </div>
            </section>
          )}

          {(view === "workspaces" || view === "automations") && <section className="empty-view"><div className="empty-view-icon" aria-hidden="true">{navigation.find((item) => item.id === view)?.glyph}</div><h2>{viewTitle}</h2><p>This module is disabled in this build until its permission and audit surfaces are complete. No background activity runs.</p></section>}
        </main>

        <aside className="inspector panel-divider-left">
          <div className="inspector-heading"><div><div className="eyebrow">Live status</div><h2>Runtime</h2></div><button className="refresh-button" type="button" onClick={() => void refreshRuntime()} aria-label="Refresh runtime status">↻</button></div>
          <section className="runtime-card card"><div className="card-heading"><span className="card-title">Inference</span><span className={`pill ${snapshot.running ? "pill-green" : "pill-muted"}`}>{snapshot.running ? "READY" : "OFFLINE"}</span></div><div className="runtime-model">{snapshot.model_name ?? "No model loaded"}</div><div className="runtime-detail">{snapshot.running ? `llama.cpp · ${snapshot.accelerator}` : "Open Models to load a GGUF file"}</div>{runtime.last_error && <div className="runtime-error">{runtime.last_error}</div>}</section>
          <section className="metrics-card card"><div className="card-heading"><span className="card-title">Runtime metrics</span><span className="metric-live"><span className={`status-dot ${snapshot.running ? "is-ready" : ""}`} /> live</span></div><dl className="metric-list"><div><dt>Accelerator</dt><dd>{snapshot.accelerator ?? "—"}</dd></div><div><dt>VRAM</dt><dd>{formatBytes(runtime.vram_bytes)}</dd></div><div><dt>Context</dt><dd>{snapshot.context_length ? `${snapshot.context_length.toLocaleString()} tokens` : "—"}</dd></div><div><dt>Generation</dt><dd>{runtime.tokens_per_second ? `${runtime.tokens_per_second.toFixed(1)} tok/s` : "—"}</dd></div><div><dt>Local port</dt><dd>{snapshot.port ?? "—"}</dd></div></dl></section>
          <section className="security-card card"><div className="card-heading"><span className="card-title">Safety boundary</span><span className="shield-icon" aria-hidden="true">◆</span></div><p>Model output cannot execute shell, file, browser, keyboard, mouse or network side effects. Tool execution remains locked behind native permission permits.</p><div className="security-status"><span className="status-dot is-ready" /> strict mode</div></section>
          <div className="inspector-bottom"><button className="stop-button" type="button" onClick={() => void handleStop()}><span aria-hidden="true">■</span><span>Stop everything</span></button></div>
        </aside>
      </div>
      <footer className="statusbar"><span><span className="status-dot is-ready" /> Local-first</span><span>Permissions: strict</span><span className="statusbar-spacer" /><span>Telemetry: off</span></footer>
    </div>
  );
}

export default App;
