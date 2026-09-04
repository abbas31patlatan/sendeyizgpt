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
import { translate, type Locale, type TranslationKey } from "./i18n";
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

type Theme = "system" | "dark" | "light";

type UiPreferences = {
  locale: Locale;
  theme: Theme;
};

type ChatBinding = {
  conversationId: string;
  assistantId: string;
};

const CONVERSATIONS_KEY = "aegis.conversations.v1";
const SETTINGS_KEY = "aegis.provider-settings.v1";
const UI_PREFERENCES_KEY = "aegis.ui-preferences.v1";

const navigation: Array<{ id: View; labelKey: TranslationKey; glyph: string }> = [
  { id: "chats", labelKey: "navChats", glyph: "◌" },
  { id: "workspaces", labelKey: "navWorkspaces", glyph: "⌘" },
  { id: "models", labelKey: "navModels", glyph: "▣" },
  { id: "automations", labelKey: "navAutomations", glyph: "◷" },
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

function newConversation(title = "New conversation"): Conversation {
  return {
    id: createId("conversation"),
    title,
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

function loadUiPreferences(): UiPreferences {
  try {
    const raw = window.localStorage.getItem(UI_PREFERENCES_KEY);
    const parsed = raw ? JSON.parse(raw) as Partial<UiPreferences> : {};
    const locale: Locale =
      parsed.locale === "tr" || parsed.locale === "en"
        ? parsed.locale
        : navigator.language.toLowerCase().startsWith("tr") ? "tr" : "en";
    const theme: Theme =
      parsed.theme === "dark" || parsed.theme === "light" || parsed.theme === "system"
        ? parsed.theme
        : "system";
    return { locale, theme };
  } catch {
    return {
      locale: navigator.language.toLowerCase().startsWith("tr") ? "tr" : "en",
      theme: "system",
    };
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
  const [uiPreferences, setUiPreferences] = useState<UiPreferences>(loadUiPreferences);
  const [draft, setDraft] = useState("");
  const [conversationQuery, setConversationQuery] = useState("");
  const [copiedMessageId, setCopiedMessageId] = useState<string | null>(null);
  const [streamingOperation, setStreamingOperation] = useState<string | null>(null);
  const [stopMessage, setStopMessage] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [modelOptions, setModelOptions] = useState<ProviderModel[]>([]);
  const [modelsLoading, setModelsLoading] = useState(false);
  const [connectionMessage, setConnectionMessage] = useState<string | null>(null);
  const operationBindings = useRef(new Map<string, ChatBinding>());
  const queuedEvents = useRef(new Map<string, ChatEvent[]>());
  const streamDeltas = useRef(
    new Map<string, { content: string; reasoning: string }>(),
  );
  const streamFrame = useRef<number | null>(null);
  const conversationSearchRef = useRef<HTMLInputElement>(null);
  const composerRef = useRef<HTMLTextAreaElement>(null);
  const tx = useCallback(
    (key: TranslationKey, values?: Record<string, string | number>) =>
      translate(uiPreferences.locale, key, values),
    [uiPreferences.locale],
  );
  const localeTag = uiPreferences.locale === "tr" ? "tr-TR" : "en-US";

  const activeConversation = useMemo(
    () => conversations.find((conversation) => conversation.id === activeConversationId) ?? null,
    [activeConversationId, conversations],
  );
  const activeMessages = activeConversation?.messages ?? [];
  const visibleConversations = useMemo(() => {
    const query = conversationQuery.trim().toLocaleLowerCase(localeTag);
    return [...conversations]
      .sort((left, right) => right.updatedAt - left.updatedAt)
      .filter((conversation) =>
        !query || conversation.title.toLocaleLowerCase(localeTag).includes(query),
      );
  }, [conversationQuery, conversations, localeTag]);
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

  const flushStreamDeltas = useCallback(() => {
    streamFrame.current = null;
    const updates: Array<ChatBinding & { content: string; reasoning: string }> = [];

    for (const [operationId, delta] of streamDeltas.current) {
      const binding = operationBindings.current.get(operationId);
      if (!binding) {
        continue;
      }
      updates.push({ ...binding, ...delta });
      streamDeltas.current.delete(operationId);
    }

    if (updates.length === 0) {
      return;
    }

    setConversations((current) =>
      current.map((conversation) => {
        const conversationUpdates = updates.filter(
          (update) => update.conversationId === conversation.id,
        );
        if (conversationUpdates.length === 0) {
          return conversation;
        }
        return {
          ...conversation,
          updatedAt: Date.now(),
          messages: conversation.messages.map((message) => {
            const update = conversationUpdates.find(
              (candidate) => candidate.assistantId === message.id,
            );
            if (!update) {
              return message;
            }
            return {
              ...message,
              content: message.content + update.content,
              reasoning: (message.reasoning ?? "") + update.reasoning,
              status: "streaming",
            };
          }),
        };
      }),
    );
  }, []);

  const scheduleStreamFlush = useCallback(() => {
    if (streamFrame.current === null) {
      streamFrame.current = window.requestAnimationFrame(flushStreamDeltas);
    }
  }, [flushStreamDeltas]);

  const processBoundChatEvent = useCallback(
    (event: ChatEvent, binding: ChatBinding) => {
      if (event.type === "token" || event.type === "reasoning") {
        const current = streamDeltas.current.get(event.operation_id) ?? {
          content: "",
          reasoning: "",
        };
        if (event.type === "token") {
          current.content += event.text;
        } else {
          current.reasoning += event.text;
        }
        streamDeltas.current.set(event.operation_id, current);
        scheduleStreamFlush();
        return;
      }

      if (streamFrame.current !== null) {
        window.cancelAnimationFrame(streamFrame.current);
        streamFrame.current = null;
      }
      flushStreamDeltas();

      if (event.type === "failed") {
        setError(event.message);
        updateConversation(binding.conversationId, (conversation) =>
          updateStoredMessage(conversation, binding.assistantId, (message) => ({
            ...message,
            content: message.content || tx("providerError", { message: event.message }),
            status: "error",
          })),
        );
      } else if (event.type === "cancelled") {
        updateConversation(binding.conversationId, (conversation) =>
          updateStoredMessage(conversation, binding.assistantId, (message) => ({
            ...message,
            content: message.content || tx("generationCancelled"),
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
        queuedEvents.current.delete(event.operation_id);
        setStreamingOperation((current) =>
          current === event.operation_id ? null : current,
        );
      }
    },
    [flushStreamDeltas, scheduleStreamFlush, tx, updateConversation],
  );

  const onChatEvent = useCallback(
    (event: ChatEvent) => {
      if (event.type === "started") {
        setStreamingOperation(event.operation_id);
        return;
      }
      const binding = operationBindings.current.get(event.operation_id);
      if (!binding) {
        const pending = queuedEvents.current.get(event.operation_id) ?? [];
        pending.push(event);
        queuedEvents.current.set(event.operation_id, pending.slice(-256));
        return;
      }
      processBoundChatEvent(event, binding);
    },
    [processBoundChatEvent],
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
    const persistenceTimer = window.setTimeout(() => {
      try {
        window.localStorage.setItem(CONVERSATIONS_KEY, JSON.stringify(conversations));
        const nonSecretSettings = {
          base_url: settings.base_url,
          model: settings.model,
          max_tokens: settings.max_tokens,
          temperature: settings.temperature,
        };
        window.localStorage.setItem(SETTINGS_KEY, JSON.stringify(nonSecretSettings));
        window.localStorage.setItem(UI_PREFERENCES_KEY, JSON.stringify(uiPreferences));
      } catch {
        // Persistence is best effort; the chat remains usable in private browsing.
      }
    }, 300);
    return () => window.clearTimeout(persistenceTimer);
  }, [conversations, settings, uiPreferences]);

  useEffect(() => {
    const media = window.matchMedia("(prefers-color-scheme: dark)");
    const applyTheme = () => {
      const resolved =
        uiPreferences.theme === "system"
          ? media.matches ? "dark" : "light"
          : uiPreferences.theme;
      document.documentElement.dataset.theme = resolved;
      document.documentElement.lang = uiPreferences.locale;
    };
    applyTheme();
    media.addEventListener("change", applyTheme);
    return () => media.removeEventListener("change", applyTheme);
  }, [uiPreferences.locale, uiPreferences.theme]);

  useEffect(() => () => {
    if (streamFrame.current !== null) {
      window.cancelAnimationFrame(streamFrame.current);
    }
  }, []);

  useEffect(() => {
    const onShortcut = (event: globalThis.KeyboardEvent) => {
      if (!(event.ctrlKey || event.metaKey)) {
        return;
      }
      if (event.key.toLowerCase() === "n") {
        event.preventDefault();
        const conversation = newConversation(tx("newConversation"));
        setConversations((current) => [conversation, ...current]);
        setActiveConversationId(conversation.id);
        setView("chats");
        window.requestAnimationFrame(() => composerRef.current?.focus());
      } else if (event.key.toLowerCase() === "k") {
        event.preventDefault();
        setView("chats");
        window.requestAnimationFrame(() => conversationSearchRef.current?.focus());
      }
    };
    window.addEventListener("keydown", onShortcut);
    return () => window.removeEventListener("keydown", onShortcut);
  }, [tx]);

  const viewTitle = useMemo(
    () => tx(navigation.find((item) => item.id === view)?.labelKey ?? "appName"),
    [tx, view],
  );

  const handleNewChat = () => {
    const conversation = newConversation(tx("newConversation"));
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
      baseConversation.messages.length === 0
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
      const binding = { conversationId, assistantId };
      operationBindings.current.set(started.operation_id, binding);
      setStreamingOperation(started.operation_id);
      const pending = queuedEvents.current.get(started.operation_id) ?? [];
      queuedEvents.current.delete(started.operation_id);
      for (const event of pending) {
        processBoundChatEvent(event, binding);
      }
      flushStreamDeltas();
    } catch (sendError) {
      const message = errorMessage(sendError);
      setError(message);
      updateConversation(conversationId, (conversation) =>
        updateStoredMessage(conversation, assistantId, (assistant) => ({
          ...assistant,
          content: tx("unableToStart", { message }),
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
      if (streamingOperation) {
        await cancelOperation(streamingOperation);
      }
      const count = await stopEverything();
      setStopMessage(
        count === 0 ? tx("noActiveOperations") : tx("operationsCancelled", { count }),
      );
    } catch {
      setStopMessage(tx("coreNotConnected"));
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
          ? tx("connectedNoModels")
          : tx("modelsAvailable", { count: models.length }),
      );
    } catch (connectionError) {
      setConnectionMessage(errorMessage(connectionError));
    } finally {
      setModelsLoading(false);
    }
  };

  const handleCopyMessage = async (message: StoredMessage) => {
    try {
      await navigator.clipboard.writeText(message.content);
      setCopiedMessageId(message.id);
      window.setTimeout(() => setCopiedMessageId(null), 1400);
    } catch {
      setError(tx("copyFailed"));
    }
  };

  const handleDeleteConversation = (conversationId: string) => {
    if (!window.confirm(tx("deleteConversationConfirm"))) {
      return;
    }
    setConversations((current) => {
      const next = current.filter((conversation) => conversation.id !== conversationId);
      if (activeConversationId === conversationId) {
        setActiveConversationId(next[0]?.id ?? null);
      }
      return next;
    });
  };

  const applyProviderPreset = (preset: "ollama" | "lmstudio") => {
    setSettings((current) => ({
      ...current,
      base_url:
        preset === "ollama"
          ? "http://127.0.0.1:11434/v1"
          : "http://127.0.0.1:1234/v1",
      model: preset === "ollama" ? current.model || "llama3.2" : "",
      api_key: "",
    }));
    setConnectionMessage(null);
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
            <div className="eyebrow">{tx("secureByConstruction")}</div>
            <h2>{tx("welcomeTitle")}</h2>
            <p>{tx("welcomeDescription")}</p>
          </section>

          <div className="starter-grid" aria-label={tx("starterPrompts")}>
            <button className="starter-card" type="button" onClick={() => setView("workspaces")}>
              <span className="starter-icon">⌁</span>
              <span>
                <strong>{tx("openProject")}</strong>
                <small>{tx("openProjectDescription")}</small>
              </span>
            </button>
            <button className="starter-card" type="button" onClick={() => setView("models")}>
              <span className="starter-icon">◈</span>
              <span>
                <strong>{tx("loadLocalModel")}</strong>
                <small>{tx("loadLocalModelDescription")}</small>
              </span>
            </button>
            <button
              className="starter-card"
              type="button"
              onClick={() => {
                setDraft(tx("permissionPrompt"));
                window.requestAnimationFrame(() => composerRef.current?.focus());
              }}
            >
              <span className="starter-icon">✦</span>
              <span>
                <strong>{tx("reviewPermissions")}</strong>
                <small>{tx("reviewPermissionsDescription")}</small>
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
              <span>{message.role === "user" ? tx("you") : tx("appName")}</span>
              <time dateTime={new Date(message.createdAt).toISOString()}>
                {new Date(message.createdAt).toLocaleTimeString(localeTag, {
                  hour: "2-digit",
                  minute: "2-digit",
                })}
              </time>
              {message.content && (
                <button
                  className="message-action"
                  type="button"
                  onClick={() => void handleCopyMessage(message)}
                  aria-label={tx("copyMessage")}
                  title={tx("copyMessage")}
                >
                  {copiedMessageId === message.id ? tx("copied") : tx("copy")}
                </button>
              )}
            </div>
            <div className="message-bubble">
              {message.reasoning && (
                <details className="reasoning-block" open={message.status === "streaming"}>
                  <summary>{tx("reasoningTrace")}</summary>
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
          <div className="eyebrow">{tx("providerRouting")}</div>
          <h2>{tx("connectModelTitle")}</h2>
        </div>
        <span className="pill pill-green">{tx("localFirst")}</span>
      </div>
      <p className="settings-intro">{tx("settingsIntro")}</p>

      <div className="provider-presets" aria-label={tx("quickSetup")}>
        <button type="button" onClick={() => applyProviderPreset("ollama")}>
          <span className="preset-mark">O</span>
          <span><strong>Ollama</strong><small>127.0.0.1:11434</small></span>
        </button>
        <button type="button" onClick={() => applyProviderPreset("lmstudio")}>
          <span className="preset-mark">L</span>
          <span><strong>LM Studio</strong><small>127.0.0.1:1234</small></span>
        </button>
      </div>

      <div className="settings-grid">
        <label>
          <span>{tx("baseUrl")}</span>
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
          <span>{tx("model")}</span>
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
          <span>{tx("apiKey")} <em>{tx("optional")}</em></span>
          <input
            type="password"
            value={settings.api_key}
            onChange={(event) =>
              setSettings((current) => ({ ...current, api_key: event.target.value }))
            }
            placeholder={tx("remoteProvidersOnly")}
            autoComplete="off"
          />
        </label>
        <label>
          <span>{tx("maxNewTokens")}</span>
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
          <span>{tx("temperature")}</span>
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
          {modelsLoading ? tx("checking") : tx("checkConnection")}
        </button>
        {connectionMessage && (
          <span className="connection-message" role="status">{connectionMessage}</span>
        )}
      </div>
      <div className="provider-help">
        <strong>{tx("privacyFirst")}</strong>
        <span>{tx("providerHelp")}</span>
      </div>
    </section>
  );

  const renderRoadmap = () => (
    <section className="empty-view roadmap-view">
      <div className="empty-view-icon" aria-hidden="true">
        {navigation.find((item) => item.id === view)?.glyph}
      </div>
      <div className="eyebrow">{tx("designedNotFaked")}</div>
      <h2>{viewTitle}</h2>
      <p>{tx("roadmapDescription")}</p>
      <button className="secondary-button" type="button" onClick={() => setView("chats")}>
        {tx("returnToChat")}
      </button>
    </section>
  );

  return (
    <div className="app-shell">
      <header className="topbar">
        <div className="brand-lockup">
          <div className="brand-mark" aria-hidden="true">A</div>
          <div>
            <div className="brand-name">{tx("appName")}</div>
            <div className="brand-subtitle">{tx("brandSubtitle")}</div>
          </div>
        </div>

        <div className="topbar-status">
          <span className={"status-dot " + (coreReady ? "is-ready" : "")} />
          <span>{coreReady ? tx("coreReady") : tx("coreUnavailable")}</span>
          <span className="topbar-divider" />
          <span className="muted">v{runtime.app_version}</span>
        </div>

        <div className="topbar-preferences">
          <label className="compact-select">
            <span>{tx("language")}</span>
            <select
              value={uiPreferences.locale}
              onChange={(event) =>
                setUiPreferences((current) => ({
                  ...current,
                  locale: event.target.value as Locale,
                }))
              }
            >
              <option value="tr">Türkçe</option>
              <option value="en">English</option>
            </select>
          </label>
          <label className="compact-select">
            <span>{tx("theme")}</span>
            <select
              value={uiPreferences.theme}
              onChange={(event) =>
                setUiPreferences((current) => ({
                  ...current,
                  theme: event.target.value as Theme,
                }))
              }
            >
              <option value="system">{tx("themeSystem")}</option>
              <option value="dark">{tx("themeDark")}</option>
              <option value="light">{tx("themeLight")}</option>
            </select>
          </label>
        </div>
      </header>

      <div className="workspace-grid">
        <aside className="sidebar panel-divider-right">
          <div className="sidebar-actions">
            <button className="new-chat-button" type="button" onClick={handleNewChat}>
              <span aria-hidden="true">＋</span>
              <span>{tx("newChat")}</span>
              <kbd>Ctrl N</kbd>
            </button>
          </div>

          <nav className="primary-nav" aria-label={tx("primaryNavigation")}>
            {navigation.map((item) => (
              <button
                className={"nav-item " + (view === item.id ? "is-active" : "")}
                key={item.id}
                type="button"
                onClick={() => setView(item.id)}
              >
                <span className="nav-glyph" aria-hidden="true">{item.glyph}</span>
                <span>{tx(item.labelKey)}</span>
              </button>
            ))}
          </nav>

          <div className="sidebar-section recent-section">
            <div className="section-label-row">
              <div className="section-label">{tx("recentChats")}</div>
              <kbd>Ctrl K</kbd>
            </div>
            <label className="conversation-search">
              <span aria-hidden="true">⌕</span>
              <input
                ref={conversationSearchRef}
                value={conversationQuery}
                onChange={(event) => setConversationQuery(event.target.value)}
                placeholder={tx("searchChats")}
                aria-label={tx("searchChats")}
              />
            </label>
            {conversations.length === 0 ? (
              <div className="empty-sidebar-state">{tx("noConversations")}</div>
            ) : visibleConversations.length === 0 ? (
              <div className="empty-sidebar-state">{tx("noSearchResults")}</div>
            ) : (
              <div className="conversation-list">
                {visibleConversations.slice(0, 12).map((conversation) => (
                  <div className="conversation-row" key={conversation.id}>
                    <button
                      className={
                        "conversation-item " +
                        (conversation.id === activeConversationId ? "is-active" : "")
                      }
                      type="button"
                      onClick={() => {
                        setActiveConversationId(conversation.id);
                        setView("chats");
                      }}
                    >
                      <span>{conversation.title}</span>
                      <small>{tx("messageCount", { count: conversation.messages.length })}</small>
                    </button>
                    <button
                      className="conversation-delete"
                      type="button"
                      onClick={() => handleDeleteConversation(conversation.id)}
                      aria-label={tx("deleteConversation")}
                      title={tx("deleteConversation")}
                    >
                      ×
                    </button>
                  </div>
                ))}
              </div>
            )}
          </div>

          <div className="sidebar-section sidebar-bottom">
            <button className="secondary-nav-item" type="button" onClick={() => setView("models")}>
              <span aria-hidden="true">⚙</span>
              <span>{tx("settings")}</span>
            </button>
            <button className="secondary-nav-item" type="button" onClick={() => void refreshRuntime()}>
              <span aria-hidden="true">?</span>
              <span>{tx("diagnostics")}</span>
            </button>
          </div>
        </aside>

        <main className="main-panel">
          <div className="content-toolbar">
            <div>
              <div className="eyebrow">{tx("workspace")}</div>
              <h1>{viewTitle}</h1>
              {view === "chats" && activeConversation && (
                <div className="toolbar-subtitle">{activeConversation.title}</div>
              )}
            </div>
            <div className="toolbar-actions">
              <span className="model-chip" title={settings.base_url}>
                <span className={"status-dot " + (coreReady ? "is-ready" : "")} />
                {settings.model || tx("noModelSelected")}
              </span>
              <button
                className="icon-button"
                type="button"
                aria-label={tx("openModelSettings")}
                title={tx("openModelSettings")}
                onClick={() => setView("models")}
              >
                ◈
              </button>
              <button
                className="icon-button"
                type="button"
                aria-label={tx("refreshRuntime")}
                title={tx("refreshRuntime")}
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
                  ref={composerRef}
                  aria-label={tx("messageComposer")}
                  value={draft}
                  onChange={(event) => setDraft(event.target.value)}
                  onKeyDown={handleComposerKeyDown}
                  placeholder={tx("askPlaceholder")}
                  rows={3}
                  maxLength={262144}
                  disabled={isStreaming}
                />
                <div className="composer-footer">
                  <div className="composer-hint">
                    <span className="lock-icon" aria-hidden="true">⌑</span>
                    {isStreaming
                      ? tx("generatingFrom", { model: settings.model || tx("selectedModel") })
                      : tx("modelEndpoint", {
                          model: settings.model || tx("noModelSelected"),
                          endpoint: settings.base_url,
                        })}
                  </div>
                  <span className="composer-count">{draft.length.toLocaleString(localeTag)}</span>
                  {isStreaming ? (
                    <button
                      className="send-button cancel-button"
                      type="button"
                      onClick={() => void handleStop()}
                      aria-label={tx("cancelGeneration")}
                      title={tx("cancelGeneration")}
                    >
                      ■
                    </button>
                  ) : (
                    <button
                      className="send-button"
                      type="submit"
                      disabled={!draft.trim() || !settings.model.trim()}
                      aria-label={tx("sendMessage")}
                      title={tx("sendMessage")}
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
              <div className="eyebrow">{tx("liveStatus")}</div>
              <h2>{tx("runtime")}</h2>
            </div>
            <button
              className="refresh-button"
              type="button"
              onClick={() => void refreshRuntime()}
              aria-label={tx("refreshRuntimeStatus")}
            >
              ↻
            </button>
          </div>

          <section className="runtime-card card">
            <div className="card-heading">
              <span className="card-title">{tx("inference")}</span>
              <span className={"pill " + (coreReady ? "pill-green" : "pill-muted")}>
                {coreReady ? tx("coreReadyUpper") : tx("offline")}
              </span>
            </div>
            <div className="runtime-model">{settings.model || tx("noModelSelected")}</div>
            <div className="runtime-detail">
              {runtime.backend_name ?? tx("openAiCompatibleProvider")}
            </div>
            {runtime.last_error && !coreReady && (
              <div className="runtime-error">{runtime.last_error}</div>
            )}
          </section>

          <section className="metrics-card card">
            <div className="card-heading">
              <span className="card-title">{tx("hardware")}</span>
              <span className="metric-live"><span className="status-dot" /> {tx("telemetry")}</span>
            </div>
            <dl className="metric-list">
              <div><dt>{tx("accelerator")}</dt><dd>{runtime.accelerator ?? "—"}</dd></div>
              <div><dt>{tx("gpu")}</dt><dd>{runtime.gpu_name ?? "—"}</dd></div>
              <div><dt>{tx("vram")}</dt><dd>{formatBytes(runtime.vram_bytes)}</dd></div>
              <div>
                <dt>{tx("context")}</dt>
                <dd>
                  {runtime.context_length
                    ? tx("tokenCount", { count: runtime.context_length.toLocaleString(localeTag) })
                    : "—"}
                </dd>
              </div>
              <div>
                <dt>{tx("generation")}</dt>
                <dd>
                  {runtime.tokens_per_second
                    ? tx("tokensPerSecond", { count: runtime.tokens_per_second.toFixed(1) })
                    : "—"}
                </dd>
              </div>
            </dl>
          </section>

          <section className="security-card card">
            <div className="card-heading">
              <span className="card-title">{tx("safetyBoundary")}</span>
              <span className="shield-icon" aria-hidden="true">◆</span>
            </div>
            <p>{tx("securityDescription")}</p>
            <div className="security-status">
              <span className="status-dot is-ready" /> {tx("sideEffectsLocked")}
            </div>
          </section>

          <div className="inspector-bottom">
            {stopMessage && <div className="stop-message" role="status">{stopMessage}</div>}
            <button className="stop-button" type="button" onClick={() => void handleStop()}>
              <span aria-hidden="true">■</span>
              <span>{tx("stopEverything")}</span>
            </button>
          </div>
        </aside>
      </div>

      <footer className="statusbar">
        <span><span className="status-dot is-ready" /> {tx("localFirstMode")}</span>
        <span>{tx("permissionsStrict")}</span>
        <span className="statusbar-spacer" />
        <span>{tx("apiKeySessionOnly")}</span>
      </footer>
    </div>
  );
}

export default App;
