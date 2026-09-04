import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type FormEvent,
  type KeyboardEvent as ReactKeyboardEvent,
} from "react";
import {
  cancelOperation,
  getRuntimeStatus,
  listProviderModels,
  listenChatEvents,
  startChat,
  stopEverything,
} from "./ipc";
import {
  formatBytes,
  initialUnavailableStatus,
  type ChatEvent,
  type ChatMessage,
  type ProviderConfig,
  type ProviderModel,
  type RuntimeStatus,
} from "./protocol";
import "./chat.css";

type View = "chats" | "workspaces" | "models" | "automations";
type MessageStatus = "streaming" | "error" | "cancelled";

type StoredMessage = ChatMessage & {
  id: string;
  createdAt: number;
  reasoning?: string;
  status?: MessageStatus;
};

type Conversation = {
  id: string;
  title: string;
  messages: StoredMessage[];
  updatedAt: number;
};

type ProviderSettings = {
  base_url: string;
  model: string;
  api_key: string;
  max_tokens: number;
  temperature: number;
};

const CONVERSATIONS_KEY = "aegis.conversations.v1";
const SETTINGS_KEY = "aegis.provider-settings.v1";

const navigation: Array<{ id: View; label: string; glyph: string }> = [
  { id: "chats", label: "Chats", glyph: "◌" },
  { id: "workspaces", label: "Workspaces", glyph: "⌘" },
  { id: "models", label: "Model library", glyph: "▣" },
  { id: "automations", label: "Automations", glyph: "◷" },
];

const defaultSettings: ProviderSettings = {
  base_url: "http://127.0.0.1:11434/v1",
  model: "llama3.2",
  api_key: "",
  max_tokens: 1024,
  temperature: 0.7,
};

function createId(prefix: string): string {
  return prefix + "-" + Date.now().toString(36) + "-" + Math.random().toString(36).slice(2, 9);
}

function newConversation(): Conversation {
  return {
    id: createId("conversation"),
    title: "New conversation",
    messages: [],
    updatedAt: Date.now(),
  };
}

function isConversation(value: unknown): value is Conversation {
  if (typeof value !== "object" || value === null) {
    return false;
  }
  const candidate = value as Partial<Conversation>;
  return (
    typeof candidate.id === "string" &&
    typeof candidate.title === "string" &&
    typeof candidate.updatedAt === "number" &&
    Array.isArray(candidate.messages)
  );
}

function loadConversations(): Conversation[] {
  try {
    const raw = window.localStorage.getItem(CONVERSATIONS_KEY);
    if (!raw) {
      return [];
    }
    const parsed: unknown = JSON.parse(raw);
    return Array.isArray(parsed) ? parsed.filter(isConversation) : [];
  } catch {
    return [];
  }
}

function loadSettings(): ProviderSettings {
  try {
    const raw = window.localStorage.getItem(SETTINGS_KEY);
    if (!raw) {
      return defaultSettings;
    }
    const parsed: unknown = JSON.parse(raw);
    if (typeof parsed !== "object" || parsed === null) {
      return defaultSettings;
    }
    const value = parsed as Partial<ProviderSettings>;
    return {
      base_url: typeof value.base_url === "string" ? value.base_url : defaultSettings.base_url,
      model: typeof value.model === "string" ? value.model : defaultSettings.model,
      api_key: "",
      max_tokens:
        typeof value.max_tokens === "number" && Number.isFinite(value.max_tokens)
          ? Math.max(1, Math.floor(value.max_tokens))
          : defaultSettings.max_tokens,
      temperature:
        typeof value.temperature === "number" && Number.isFinite(value.temperature)
          ? Math.min(2, Math.max(0, value.temperature))
          : defaultSettings.temperature,
    };
  } catch {
    return defaultSettings;
  }
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function updateStoredMessage(
  conversation: Conversation,
  messageId: string,
  update: (message: StoredMessage) => StoredMessage,
): Conversation {
  return {
    ...conversation,
    messages: conversation.messages.map((message) =>
      message.id === messageId ? update(message) : message,
    ),
    updatedAt: Date.now(),
  };
}

function App() {
  const [view, setView] = useState<View>("chats");
  const [runtime, setRuntime] = useState<RuntimeStatus>(initialUnavailableStatus);
  const [conversations, setConversations] = useState<Conversation[]>(loadConversations);
  const [activeConversationId, setActiveConversationId] = useState<string | null>(() => {
    const saved = loadConversations();
    return saved[0]?.id ?? null;
  });
  const [settings, setSettings] = useState<ProviderSettings>(loadSettings);
  const [draft, setDraft] = useState("");
  const [streamingOperation, setStreamingOperation] = useState<string | null>(null);
  const [stopMessage, setStopMessage] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [modelOptions, setModelOptions] = useState<ProviderModel[]>([]);
  const [modelsLoading, setModelsLoading] = useState(false);
  const [connectionMessage, setConnectionMessage] = useState<string | null>(null);
  const operationBindings = useRef(
    new Map<string, { conversationId: string; assistantId: string }>(),
  );

  const activeConversation = useMemo(
    () => conversations.find((conversation) => conversation.id === activeConversationId) ?? null,
    [activeConversationId, conversations],
  );
  const activeMessages = activeConversation?.messages ?? [];
  const isStreaming = streamingOperation !== null;
  const coreReady = runtime.core_state === "ready";

  const refreshRuntime = useCallback(async () => {
    try {
      setRuntime(await getRuntimeStatus());
    } catch {
      setRuntime(initialUnavailableStatus);
    }
  }, []);

  const updateConversation = useCallback(
    (conversationId: string, updater: (conversation: Conversation) => Conversation) => {
      setConversations((current) =>
        current.map((conversation) =>
          conversation.id === conversationId ? updater(conversation) : conversation,
        ),
      );
    },
    [],
  );

  const onChatEvent = useCallback(
    (event: ChatEvent) => {
      if (event.type === "started") {
        setStreamingOperation(event.operation_id);
        return;
      }
      const binding = operationBindings.current.get(event.operation_id);
      if (!binding) {
        return;
      }

      if (event.type === "token") {
        updateConversation(binding.conversationId, (conversation) =>
          updateStoredMessage(conversation, binding.assistantId, (message) => ({
            ...message,
            content: message.content + event.text,
            status: "streaming",
          })),
        );
      } else if (event.type === "reasoning") {
        updateConversation(binding.conversationId, (conversation) =>
          updateStoredMessage(conversation, binding.assistantId, (message) => ({
            ...message,
            reasoning: (message.reasoning ?? "") + event.text,
            status: "streaming",
          })),
        );
      } else if (event.type === "failed") {
        setError(event.message);
        updateConversation(binding.conversationId, (conversation) =>
          updateStoredMessage(conversation, binding.assistantId, (message) => ({
            ...message,
            content:
              message.content || "Provider error: " + event.message,
            status: "error",
          })),
        );
      } else if (event.type === "cancelled") {
        updateConversation(binding.conversationId, (conversation) =>
          updateStoredMessage(conversation, binding.assistantId, (message) => ({
            ...message,
            content: message.content || "Generation cancelled.",
            status: "cancelled",
          })),
        );
      } else if (event.type === "finished") {
        updateConversation(binding.conversationId, (conversation) =>
          updateStoredMessage(conversation, binding.assistantId, (message) => {
            const completed = { ...message };
            delete completed.status;
            return completed;
          }),
        );
      }

      if (
        event.type === "finished" ||
        event.type === "failed" ||
        event.type === "cancelled"
      ) {
        operationBindings.current.delete(event.operation_id);
        setStreamingOperation((current) =>
          current === event.operation_id ? null : current,
        );
      }
    },
    [updateConversation],
  );

  useEffect(() => {
    void refreshRuntime();
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void listenChatEvents(onChatEvent).then((stopListening) => {
      if (disposed) {
        stopListening();
      } else {
        unlisten = stopListening;
      }
    }).catch(() => {
      // The Vite preview is intentionally usable without the Tauri event bridge.
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [onChatEvent, refreshRuntime]);

  useEffect(() => {
    try {
      window.localStorage.setItem(CONVERSATIONS_KEY, JSON.stringify(conversations));
      const nonSecretSettings = {
        base_url: settings.base_url,
        model: settings.model,
        max_tokens: settings.max_tokens,
        temperature: settings.temperature,
      };
      window.localStorage.setItem(SETTINGS_KEY, JSON.stringify(nonSecretSettings));
    } catch {
      // Persistence is best effort; the chat remains usable in private browsing.
    }
  }, [conversations, settings]);

  useEffect(() => {
    const onShortcut = (event: globalThis.KeyboardEvent) => {
      if (!(event.ctrlKey || event.metaKey)) {
        return;
      }
      if (event.key.toLowerCase() === "n") {
        event.preventDefault();
        const conversation = newConversation();
        setConversations((current) => [conversation, ...current]);
        setActiveConversationId(conversation.id);
        setView("chats");
      } else if (event.key.toLowerCase() === "k") {
        event.preventDefault();
        setView("chats");
        setDraft("");
      }
    };
    window.addEventListener("keydown", onShortcut);
    return () => window.removeEventListener("keydown", onShortcut);
  }, []);

  const viewTitle = useMemo(
    () => navigation.find((item) => item.id === view)?.label ?? "Aegis AI",
    [view],
  );

  const handleNewChat = () => {
    const conversation = newConversation();
    setConversations((current) => [conversation, ...current]);
    setActiveConversationId(conversation.id);
    setView("chats");
    setDraft("");
    setError(null);
  };

  const handleSend = async () => {
    const text = draft.trim();
    if (!text || isStreaming) {
      return;
    }

    const existingMessages = activeConversation?.messages ?? [];
    const history: ChatMessage[] = existingMessages
      .filter((message) => message.content.trim().length > 0)
      .map((message) => ({
        role: message.role,
        content: message.content,
      }));
    history.push({ role: "user", content: text });

    const conversationId = activeConversation?.id ?? createId("conversation");
    const assistantId = createId("message");
    const now = Date.now();
    const userMessage: StoredMessage = {
      id: createId("message"),
      role: "user",
      content: text,
      createdAt: now,
    };
    const assistantMessage: StoredMessage = {
      id: assistantId,
      role: "assistant",
      content: "",
      createdAt: now + 1,
      status: "streaming",
    };
    const baseConversation: Conversation = activeConversation ?? {
      id: conversationId,
      title: "New conversation",
      messages: [],
      updatedAt: now,
    };
    const title =
      baseConversation.title === "New conversation"
        ? text.slice(0, 48) + (text.length > 48 ? "…" : "")
        : baseConversation.title;
    const nextConversation: Conversation = {
      ...baseConversation,
      title,
      messages: [...baseConversation.messages, userMessage, assistantMessage],
      updatedAt: now,
    };

    setConversations((current) => {
      const exists = current.some((conversation) => conversation.id === conversationId);
      return exists
        ? current.map((conversation) =>
            conversation.id === conversationId ? nextConversation : conversation,
          )
        : [nextConversation, ...current];
    });
    setActiveConversationId(conversationId);
    setDraft("");
    setError(null);
    setStopMessage(null);

    const provider: ProviderConfig = {
      base_url: settings.base_url.trim(),
      model: settings.model.trim(),
      api_key: settings.api_key.trim() || undefined,
    };

    try {
      const started = await startChat({
        provider,
        messages: history,
        max_tokens: settings.max_tokens,
        temperature: settings.temperature,
      });
      operationBindings.current.set(started.operation_id, {
        conversationId,
        assistantId,
      });
      setStreamingOperation(started.operation_id);
    } catch (sendError) {
      const message = errorMessage(sendError);
      setError(message);
      updateConversation(conversationId, (conversation) =>
        updateStoredMessage(conversation, assistantId, (assistant) => ({
          ...assistant,
          content: "Unable to start generation: " + message,
          status: "error",
        })),
      );
    }
  };

  const handleSubmit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    void handleSend();
  };

  const handleComposerKeyDown = (event: ReactKeyboardEvent<HTMLTextAreaElement>) => {
    if (event.key === "Enter" && !event.shiftKey && !event.nativeEvent.isComposing) {
      event.preventDefault();
      void handleSend();
    }
  };

  const handleStop = async () => {
    setStopMessage(null);
    try {
      const count = await stopEverything();
      setStopMessage(count === 0 ? "No active operations." : count + " operation(s) cancelled.");
    } catch {
      setStopMessage("Desktop core is not connected.");
    }
  };

  const handleCheckConnection = async () => {
    setModelsLoading(true);
    setConnectionMessage(null);
    setModelOptions([]);
    try {
      const models = await listProviderModels({
        base_url: settings.base_url.trim(),
        model: settings.model.trim() || "default",
        api_key: settings.api_key.trim() || undefined,
      });
      setModelOptions(models);
      if (!settings.model.trim() && models[0]) {
        setSettings((current) => ({ ...current, model: models[0].id }));
      }
      setConnectionMessage(
        models.length === 0
          ? "Connected, but this provider reported no models."
          : models.length + " model(s) available.",
      );
    } catch (connectionError) {
      setConnectionMessage(errorMessage(connectionError));
    } finally {
      setModelsLoading(false);
    }
  };

  const renderChat = () => {
    if (activeMessages.length === 0) {
      return (
        <>
          <section className="welcome-block">
            <div className="welcome-orbit" aria-hidden="true">
              <span className="orbit-dot orbit-dot-one" />
              <span className="orbit-dot orbit-dot-two" />
              <div className="orbit-core">A</div>
            </div>
            <div className="eyebrow">Secure by construction</div>
            <h2>What are you working on?</h2>
            <p>
              Aegis keeps models, agents and tools separate. No computer action can run
              without a native permission decision.
            </p>
          </section>

          <div className="starter-grid" aria-label="Starter prompts">
            <button className="starter-card" type="button" onClick={() => setView("workspaces")}>
              <span className="starter-icon">⌁</span>
              <span>
                <strong>Open a project</strong>
                <small>Choose a workspace and keep its context local</small>
              </span>
            </button>
            <button className="starter-card" type="button" onClick={() => setView("models")}>
              <span className="starter-icon">◈</span>
              <span>
                <strong>Load a local model</strong>
                <small>Connect Ollama, LM Studio or any OpenAI-compatible server</small>
              </span>
            </button>
            <button
              className="starter-card"
              type="button"
              onClick={() => setDraft("Explain Aegis's permission boundary and its next safe step.")}
            >
              <span className="starter-icon">✦</span>
              <span>
                <strong>Review permissions</strong>
                <small>Ask about the broker before granting a tool capability</small>
              </span>
            </button>
          </div>
        </>
      );
    }

    return (
      <section className="chat-panel" aria-live="polite">
        {activeMessages.map((message) => (
          <article
            className={
              "chat-message " +
              (message.role === "user" ? "is-user " : "is-assistant ") +
              (message.status ? "is-" + message.status : "")
            }
            key={message.id}
          >
            <div className="message-meta">
              <span>{message.role === "user" ? "You" : "Aegis"}</span>
              <time dateTime={new Date(message.createdAt).toISOString()}>
                {new Date(message.createdAt).toLocaleTimeString([], {
                  hour: "2-digit",
                  minute: "2-digit",
                })}
              </time>
            </div>
            <div className="message-bubble">
              {message.reasoning && (
                <details className="reasoning-block" open={message.status === "streaming"}>
                  <summary>Reasoning trace</summary>
                  <div>{message.reasoning}</div>
                </details>
              )}
              <div className="message-content">
                {message.content || (message.status === "streaming" ? "…" : "")}
              </div>
            </div>
          </article>
        ))}
      </section>
    );
  };

  const renderModels = () => (
    <section className="settings-panel card">
      <div className="settings-heading">
        <div>
          <div className="eyebrow">Provider routing</div>
          <h2>Connect a local or compatible model</h2>
        </div>
        <span className="pill pill-green">LOCAL-FIRST</span>
      </div>
      <p className="settings-intro">
        Requests are sent from the native process to the endpoint below. The API key stays in
        memory for this session and is never written to browser storage.
      </p>
      <div className="settings-grid">
        <label>
          <span>Base URL</span>
          <input
            value={settings.base_url}
            onChange={(event) =>
              setSettings((current) => ({ ...current, base_url: event.target.value }))
            }
            placeholder="http://127.0.0.1:11434/v1"
            spellCheck={false}
          />
        </label>
        <label>
          <span>Model</span>
          <input
            list="provider-models"
            value={settings.model}
            onChange={(event) =>
              setSettings((current) => ({ ...current, model: event.target.value }))
            }
            placeholder="llama3.2"
            spellCheck={false}
          />
          <datalist id="provider-models">
            {modelOptions.map((model) => <option key={model.id} value={model.id} />)}
          </datalist>
        </label>
        <label>
          <span>API key <em>optional</em></span>
          <input
            type="password"
            value={settings.api_key}
            onChange={(event) =>
              setSettings((current) => ({ ...current, api_key: event.target.value }))
            }
            placeholder="Only for remote providers"
            autoComplete="off"
          />
        </label>
        <label>
          <span>Max new tokens</span>
          <input
            type="number"
            min={1}
            max={131072}
            value={settings.max_tokens}
            onChange={(event) =>
              setSettings((current) => ({
                ...current,
                max_tokens: Math.min(131072, Math.max(1, Number(event.target.value) || 1)),
              }))
            }
          />
        </label>
        <label>
          <span>Temperature</span>
          <input
            type="number"
            min={0}
            max={2}
            step={0.1}
            value={settings.temperature}
            onChange={(event) =>
              setSettings((current) => ({
                ...current,
                temperature: Math.min(2, Math.max(0, Number(event.target.value) || 0)),
              }))
            }
          />
        </label>
      </div>
      <div className="settings-actions">
        <button
          className="primary-button"
          type="button"
          onClick={() => void handleCheckConnection()}
          disabled={modelsLoading || !settings.base_url.trim()}
        >
          {modelsLoading ? "Checking…" : "Check connection"}
        </button>
        {connectionMessage && <span className="connection-message" role="status">{connectionMessage}</span>}
      </div>
      <div className="provider-help">
        <strong>Compatible local servers</strong>
        <span>Ollama: http://127.0.0.1:11434/v1 · LM Studio: http://127.0.0.1:1234/v1</span>
      </div>
    </section>
  );

  const renderRoadmap = () => (
    <section className="empty-view roadmap-view">
      <div className="empty-view-icon" aria-hidden="true">
        {navigation.find((item) => item.id === view)?.glyph}
      </div>
      <div className="eyebrow">Designed, not faked</div>
      <h2>{viewTitle}</h2>
      <p>
        This surface is connected to the product roadmap. The native contracts are in place;
        the next implementation will add scoped workspaces, audit views and event automation.
      </p>
      <button className="secondary-button" type="button" onClick={() => setView("chats")}>
        Return to chat
      </button>
    </section>
  );

  return (
    <div className="app-shell">
      <header className="topbar">
        <div className="brand-lockup">
          <div className="brand-mark" aria-hidden="true">A</div>
          <div>
            <div className="brand-name">Aegis AI</div>
            <div className="brand-subtitle">local work environment</div>
          </div>
        </div>
        <div className="topbar-status">
          <span className={"status-dot " + (coreReady ? "is-ready" : "")} />
          <span>{coreReady ? "Core ready" : "Core unavailable"}</span>
          <span className="topbar-divider" />
          <span className="muted">v{runtime.app_version}</span>
        </div>
        <button
          className="avatar-button"
          type="button"
          aria-label="Open provider settings"
          onClick={() => setView("models")}
        >
          U
        </button>
      </header>

      <div className="workspace-grid">
        <aside className="sidebar panel-divider-right">
          <div className="sidebar-actions">
            <button className="new-chat-button" type="button" onClick={handleNewChat}>
              <span aria-hidden="true">+</span>
              <span>New chat</span>
              <kbd>Ctrl K</kbd>
            </button>
          </div>

          <nav className="primary-nav" aria-label="Primary navigation">
            {navigation.map((item) => (
              <button
                className={"nav-item " + (view === item.id ? "is-active" : "")}
                key={item.id}
                type="button"
                onClick={() => setView(item.id)}
              >
                <span className="nav-glyph" aria-hidden="true">{item.glyph}</span>
                <span>{item.label}</span>
              </button>
            ))}
          </nav>

          <div className="sidebar-section">
            <div className="section-label">Recent chats</div>
            {conversations.length === 0 ? (
              <div className="empty-sidebar-state">No conversations yet</div>
            ) : (
              <div className="conversation-list">
                {conversations.slice(0, 8).map((conversation) => (
                  <button
                    className={
                      "conversation-item " +
                      (conversation.id === activeConversationId ? "is-active" : "")
                    }
                    type="button"
                    key={conversation.id}
                    onClick={() => {
                      setActiveConversationId(conversation.id);
                      setView("chats");
                    }}
                  >
                    <span>{conversation.title}</span>
                    <small>{conversation.messages.length} msg</small>
                  </button>
                ))}
              </div>
            )}
          </div>

          <div className="sidebar-section sidebar-bottom">
            <button className="secondary-nav-item" type="button" onClick={() => setView("models")}>
              <span aria-hidden="true">⚙</span>
              <span>Settings</span>
            </button>
            <button className="secondary-nav-item" type="button" onClick={() => void refreshRuntime()}>
              <span aria-hidden="true">?</span>
              <span>Diagnostics</span>
            </button>
          </div>
        </aside>

        <main className="main-panel">
          <div className="content-toolbar">
            <div>
              <div className="eyebrow">Workspace</div>
              <h1>{viewTitle}</h1>
              {view === "chats" && activeConversation && (
                <div className="toolbar-subtitle">{activeConversation.title}</div>
              )}
            </div>
            <div className="toolbar-actions">
              <button
                className="icon-button"
                type="button"
                aria-label="Open model settings"
                title="Open model settings"
                onClick={() => setView("models")}
              >
                ⌕
              </button>
              <button
                className="icon-button"
                type="button"
                aria-label="Refresh runtime"
                title="Refresh runtime"
                onClick={() => void refreshRuntime()}
              >
                ↻
              </button>
            </div>
          </div>

          {view === "chats" && renderChat()}
          {view === "models" && renderModels()}
          {(view === "workspaces" || view === "automations") && renderRoadmap()}

          {view === "chats" && (
            <>
              {error && <div className="error-banner" role="alert">{error}</div>}
              <form className="composer-wrap" onSubmit={handleSubmit}>
                <textarea
                  aria-label="Message composer"
                  value={draft}
                  onChange={(event) => setDraft(event.target.value)}
                  onKeyDown={handleComposerKeyDown}
                  placeholder="Ask Aegis anything…"
                  rows={3}
                  disabled={isStreaming}
                />
                <div className="composer-footer">
                  <div className="composer-hint">
                    <span className="lock-icon" aria-hidden="true">⌑</span>
                    {isStreaming
                      ? "Generating from " + (settings.model || "selected model")
                      : (settings.model || "No model selected") + " · " + settings.base_url}
                  </div>
                  {isStreaming ? (
                    <button
                      className="send-button cancel-button"
                      type="button"
                      onClick={() => void handleStop()}
                      aria-label="Cancel generation"
                    >
                      ■
                    </button>
                  ) : (
                    <button
                      className="send-button"
                      type="submit"
                      disabled={!draft.trim() || !settings.model.trim()}
                      aria-label="Send message"
                    >
                      ↑
                    </button>
                  )}
                </div>
              </form>
            </>
          )}
        </main>

        <aside className="inspector panel-divider-left">
          <div className="inspector-heading">
            <div>
              <div className="eyebrow">Live status</div>
              <h2>Runtime</h2>
            </div>
            <button
              className="refresh-button"
              type="button"
              onClick={() => void refreshRuntime()}
              aria-label="Refresh runtime status"
            >
              ↻
            </button>
          </div>

          <section className="runtime-card card">
            <div className="card-heading">
              <span className="card-title">Inference</span>
              <span className={"pill " + (coreReady ? "pill-green" : "pill-muted")}>
                {coreReady ? "CORE READY" : "OFFLINE"}
              </span>
            </div>
            <div className="runtime-model">{settings.model || "No model selected"}</div>
            <div className="runtime-detail">
              {runtime.backend_name ?? "OpenAI-compatible provider"}
            </div>
            {runtime.last_error && !coreReady && <div className="runtime-error">{runtime.last_error}</div>}
          </section>

          <section className="metrics-card card">
            <div className="card-heading">
              <span className="card-title">Hardware</span>
              <span className="metric-live"><span className="status-dot" /> telemetry</span>
            </div>
            <dl className="metric-list">
              <div><dt>Accelerator</dt><dd>{runtime.accelerator ?? "—"}</dd></div>
              <div><dt>GPU</dt><dd>{runtime.gpu_name ?? "—"}</dd></div>
              <div><dt>VRAM</dt><dd>{formatBytes(runtime.vram_bytes)}</dd></div>
              <div><dt>Context</dt><dd>{runtime.context_length ? runtime.context_length.toLocaleString() + " tokens" : "—"}</dd></div>
              <div><dt>Generation</dt><dd>{runtime.tokens_per_second ? runtime.tokens_per_second.toFixed(1) + " tok/s" : "—"}</dd></div>
            </dl>
          </section>

          <section className="security-card card">
            <div className="card-heading">
              <span className="card-title">Safety boundary</span>
              <span className="shield-icon" aria-hidden="true">◆</span>
            </div>
            <p>Agent proposals are inert until the Permission Broker issues a one-time permit.</p>
            <div className="security-status"><span className="status-dot is-ready" /> side effects locked</div>
          </section>

          <div className="inspector-bottom">
            {stopMessage && <div className="stop-message" role="status">{stopMessage}</div>}
            <button className="stop-button" type="button" onClick={() => void handleStop()}>
              <span aria-hidden="true">■</span>
              <span>Stop everything</span>
            </button>
          </div>
        </aside>
      </div>

      <footer className="statusbar">
        <span><span className="status-dot is-ready" /> Local-first mode</span>
        <span>Permissions: strict</span>
        <span className="statusbar-spacer" />
        <span>API key is session-only</span>
      </footer>
    </div>
  );
}

export default App;
