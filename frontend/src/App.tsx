import { FormEvent, useCallback, useEffect, useMemo, useState } from "react";
import { getRuntimeStatus, stopEverything } from "./ipc";
import { formatBytes, initialUnavailableStatus, type RuntimeStatus } from "./protocol";

type View = "chats" | "workspaces" | "models" | "automations";

const navigation: Array<{ id: View; label: string; glyph: string }> = [
  { id: "chats", label: "Chats", glyph: "◌" },
  { id: "workspaces", label: "Workspaces", glyph: "⌘" },
  { id: "models", label: "Model library", glyph: "▣" },
  { id: "automations", label: "Automations", glyph: "◷" },
];

function App() {
  const [view, setView] = useState<View>("chats");
  const [runtime, setRuntime] = useState<RuntimeStatus>(initialUnavailableStatus);
  const [draft, setDraft] = useState("");
  const [stopMessage, setStopMessage] = useState<string | null>(null);

  const refreshRuntime = useCallback(async () => {
    try {
      setRuntime(await getRuntimeStatus());
    } catch {
      setRuntime(initialUnavailableStatus);
    }
  }, []);

  useEffect(() => {
    void refreshRuntime();
  }, [refreshRuntime]);

  const viewTitle = useMemo(
    () => navigation.find((item) => item.id === view)?.label ?? "Aegis AI",
    [view],
  );

  const handleSubmit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    setDraft("");
  };

  const handleStop = async () => {
    setStopMessage(null);
    try {
      const count = await stopEverything();
      setStopMessage(count === 0 ? "No active operations." : `${count} operation(s) cancelled.`);
    } catch {
      setStopMessage("Desktop core is not connected.");
    }
  };

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
          <span className={`status-dot ${runtime.core_state === "ready" ? "is-ready" : ""}`} />
          <span>{runtime.core_state === "ready" ? "Core ready" : "Core unavailable"}</span>
          <span className="topbar-divider" />
          <span className="muted">v{runtime.app_version}</span>
        </div>
        <button className="avatar-button" type="button" aria-label="Profile settings (coming soon)" disabled>U</button>
      </header>

      <div className="workspace-grid">
        <aside className="sidebar panel-divider-right">
          <div className="sidebar-actions">
            <button className="new-chat-button" type="button" onClick={() => setView("chats")}>
              <span aria-hidden="true">+</span>
              <span>New chat</span>
              <kbd>Ctrl K</kbd>
            </button>
          </div>

          <nav className="primary-nav" aria-label="Primary navigation">
            {navigation.map((item) => (
              <button
                className={`nav-item ${view === item.id ? "is-active" : ""}`}
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
            <div className="empty-sidebar-state">No conversations yet</div>
          </div>

          <div className="sidebar-section sidebar-bottom">
            <button className="secondary-nav-item" type="button" disabled>
              <span aria-hidden="true">⚙</span>
              <span>Settings</span>
            </button>
            <button className="secondary-nav-item" type="button" disabled>
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
            </div>
            <div className="toolbar-actions">
              <button className="icon-button" type="button" aria-label="Search (coming soon)" title="Search (coming soon)" disabled>⌕</button>
              <button className="icon-button" type="button" aria-label="More options (coming soon)" title="More options (coming soon)" disabled>•••</button>
            </div>
          </div>

          {view === "chats" ? (
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
                <button className="starter-card" type="button" disabled>
                  <span className="starter-icon">⌁</span>
                  <span>
                    <strong>Open a project</strong>
                    <small>Connect a workspace in the next milestone</small>
                  </span>
                </button>
                <button className="starter-card" type="button" disabled>
                  <span className="starter-icon">◈</span>
                  <span>
                    <strong>Load a local model</strong>
                    <small>GGUF and Vulkan runtime coming next</small>
                  </span>
                </button>
                <button className="starter-card" type="button" disabled>
                  <span className="starter-icon">✦</span>
                  <span>
                    <strong>Review permissions</strong>
                    <small>Tool approval surface is being wired</small>
                  </span>
                </button>
              </div>

              <form className="composer-wrap" onSubmit={handleSubmit}>
                <textarea
                  aria-label="Message composer"
                  value={draft}
                  onChange={(event) => setDraft(event.target.value)}
                  placeholder="Ask Aegis anything…"
                  rows={3}
                  disabled
                />
                <div className="composer-footer">
                  <div className="composer-hint">
                    <span className="lock-icon" aria-hidden="true">⌑</span>
                    Inference worker not connected
                  </div>
                  <button className="send-button" type="submit" disabled={!draft.trim()} aria-label="Send message">
                    ↑
                  </button>
                </div>
              </form>
            </>
          ) : (
            <section className="empty-view">
              <div className="empty-view-icon" aria-hidden="true">{navigation.find((item) => item.id === view)?.glyph}</div>
              <h2>{viewTitle}</h2>
              <p>This surface is reserved for its milestone implementation. No placeholder data is shown.</p>
            </section>
          )}
        </main>

        <aside className="inspector panel-divider-left">
          <div className="inspector-heading">
            <div>
              <div className="eyebrow">Live status</div>
              <h2>Runtime</h2>
            </div>
            <button className="refresh-button" type="button" onClick={() => void refreshRuntime()} aria-label="Refresh runtime status">↻</button>
          </div>

          <section className="runtime-card card">
            <div className="card-heading">
              <span className="card-title">Inference</span>
              <span className={`pill ${runtime.core_state === "ready" ? "pill-green" : "pill-muted"}`}>
                {runtime.core_state === "ready" ? "READY" : "OFFLINE"}
              </span>
            </div>
            <div className="runtime-model">{runtime.model_name ?? "No model loaded"}</div>
            <div className="runtime-detail">{runtime.backend_name ?? "Backend not selected"}</div>
            {runtime.last_error && <div className="runtime-error">{runtime.last_error}</div>}
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
              <div><dt>Context</dt><dd>{runtime.context_length ? `${runtime.context_length.toLocaleString()} tokens` : "—"}</dd></div>
              <div><dt>Generation</dt><dd>{runtime.tokens_per_second ? `${runtime.tokens_per_second.toFixed(1)} tok/s` : "—"}</dd></div>
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
        <span>Telemetry off until enabled</span>
      </footer>
    </div>
  );
}

export default App;
