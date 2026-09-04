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
  deleteModelLibrary,
  deletePersistedConversation,
  deletePersistedWorkspace,
  estimateModelLoad,
  getRuntimeStatus,
  inspectProvider,
  listenChatEvents,
  loadLocalModels,
  loadModelLibraries,
  loadModelProfiles,
  loadPersistedConversations,
  loadPersistedWorkspaces,
  saveModelLibrary,
  saveModelProfile,
  savePersistedConversation,
  savePersistedWorkspace,
  scanModelLibrary,
  startChat,
  stopEverything,
  validateWorkspacePath,
} from "./ipc";
import {
  formatBytes,
  initialUnavailableStatus,
  type ChatEvent,
  type LoadPreset,
  type LocalModel,
  type ModelLibrary,
  type ModelLoadEstimate,
  type ModelProfile,
  type ModelScanSummary,
  type ChatMessage,
  type PersistedConversation,
  type PersistedWorkspace,
  type ProviderConfig,
  type ProviderDiagnostics,
  type ProviderModel,
  type RuntimeStatus,
  type WorkspacePathDiagnostics,
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

type Workspace = PersistedWorkspace;

type ProviderSettings = {
  base_url: string;
  model: string;
  api_key: string;
  max_tokens: number;
  temperature: number;
  system_prompt: string;
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
const WORKSPACES_KEY = "aegis.workspaces.v1";

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
  system_prompt: "",
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
      system_prompt:
        typeof value.system_prompt === "string"
          ? value.system_prompt.slice(0, 16_384)
          : defaultSettings.system_prompt,
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

function isWorkspace(value: unknown): value is Workspace {
  if (typeof value !== "object" || value === null) {
    return false;
  }
  const candidate = value as Partial<Workspace>;
  return (
    typeof candidate.id === "string" &&
    typeof candidate.name === "string" &&
    typeof candidate.rootPath === "string" &&
    typeof candidate.createdAt === "number" &&
    typeof candidate.updatedAt === "number"
  );
}

function loadLocalWorkspaces(): Workspace[] {
  try {
    const raw = window.localStorage.getItem(WORKSPACES_KEY);
    if (!raw) {
      return [];
    }
    const parsed: unknown = JSON.parse(raw);
    return Array.isArray(parsed) ? parsed.filter(isWorkspace) : [];
  } catch {
    return [];
  }
}

function fromPersistedConversation(value: PersistedConversation): Conversation {
  return {
    id: value.id,
    title: value.title,
    updatedAt: value.updatedAt,
    messages: value.messages.map((message) => ({
      id: message.id,
      role: message.role,
      content: message.content,
      reasoning: message.reasoning ?? undefined,
      createdAt: message.createdAt,
    })),
  };
}

function toPersistedConversation(value: Conversation): PersistedConversation {
  return {
    id: value.id,
    title: value.title,
    updatedAt: value.updatedAt,
    messages: value.messages.map((message) => ({
      id: message.id,
      role: message.role,
      content: message.content,
      reasoning: message.reasoning ?? null,
      createdAt: message.createdAt,
    })),
  };
}

function mergeConversations(...groups: Conversation[][]): Conversation[] {
  const byId = new Map<string, Conversation>();
  for (const group of groups) {
    for (const conversation of group) {
      const current = byId.get(conversation.id);
      if (!current || conversation.updatedAt >= current.updatedAt) {
        byId.set(conversation.id, conversation);
      }
    }
  }
  return [...byId.values()].sort((left, right) => right.updatedAt - left.updatedAt);
}

function safeExportName(value: string): string {
  const normalized = value
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, 64);
  return normalized || "aegis-conversation";
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function formatParameterCount(value: number | null, localeTag: string): string {
  if (value === null) {
    return "—";
  }
  if (value >= 1_000_000_000_000) {
    return (value / 1_000_000_000_000).toFixed(1) + "T";
  }
  if (value >= 1_000_000_000) {
    return (value / 1_000_000_000).toFixed(1) + "B";
  }
  if (value >= 1_000_000) {
    return (value / 1_000_000).toFixed(1) + "M";
  }
  return value.toLocaleString(localeTag);
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
  const [workspaces, setWorkspaces] = useState<Workspace[]>(loadLocalWorkspaces);
  const [workspaceName, setWorkspaceName] = useState("");
  const [workspacePath, setWorkspacePath] = useState("");
  const [workspaceDiagnostics, setWorkspaceDiagnostics] =
    useState<WorkspacePathDiagnostics | null>(null);
  const [workspaceMessage, setWorkspaceMessage] = useState<string | null>(null);
  const [workspaceBusy, setWorkspaceBusy] = useState(false);
  const [modelLibraries, setModelLibraries] = useState<ModelLibrary[]>([]);
  const [localModels, setLocalModels] = useState<LocalModel[]>([]);
  const [modelProfiles, setModelProfiles] = useState<ModelProfile[]>([]);
  const [modelLibraryName, setModelLibraryName] = useState("");
  const [modelLibraryPath, setModelLibraryPath] = useState("");
  const [modelLibraryDiagnostics, setModelLibraryDiagnostics] =
    useState<WorkspacePathDiagnostics | null>(null);
  const [modelLibraryMessage, setModelLibraryMessage] = useState<string | null>(null);
  const [modelLibraryBusy, setModelLibraryBusy] = useState(false);
  const [modelScanSummary, setModelScanSummary] = useState<ModelScanSummary | null>(null);
  const [selectedLocalModelId, setSelectedLocalModelId] = useState<string | null>(null);
  const [loadPreset, setLoadPreset] = useState<LoadPreset>("balanced");
  const [loadEstimate, setLoadEstimate] = useState<ModelLoadEstimate | null>(null);
  const [profileMessage, setProfileMessage] = useState<string | null>(null);
  const [persistenceHydrated, setPersistenceHydrated] = useState(false);
  const [workspacesHydrated, setWorkspacesHydrated] = useState(false);
  const [draft, setDraft] = useState("");
  const [conversationQuery, setConversationQuery] = useState("");
  const [copiedMessageId, setCopiedMessageId] = useState<string | null>(null);
  const [streamingOperation, setStreamingOperation] = useState<string | null>(null);
  const [stopMessage, setStopMessage] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [modelOptions, setModelOptions] = useState<ProviderModel[]>([]);
  const [modelsLoading, setModelsLoading] = useState(false);
  const [providerDiagnostics, setProviderDiagnostics] =
    useState<ProviderDiagnostics | null>(null);
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
  const selectedProviderModel = useMemo(
    () => modelOptions.find((model) => model.id === settings.model.trim()) ?? null,
    [modelOptions, settings.model],
  );

  const selectedLocalModel = useMemo(
    () => localModels.find((model) => model.id === selectedLocalModelId) ?? null,
    [localModels, selectedLocalModelId],
  );

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
    let disposed = false;
    void loadPersistedConversations()
      .then((stored) => {
        if (disposed) {
          return;
        }
        const restored = stored.map(fromPersistedConversation);
        if (restored.length > 0) {
          setConversations((current) => mergeConversations(restored, current));
          setActiveConversationId((current) => current ?? restored[0]?.id ?? null);
        }
        setPersistenceHydrated(true);
      })
      .catch(() => {
        if (!disposed) {
          setPersistenceHydrated(true);
        }
      });
    return () => {
      disposed = true;
    };
  }, []);

  useEffect(() => {
    let disposed = false;
    void loadPersistedWorkspaces()
      .then((stored) => {
        if (disposed) {
          return;
        }
        if (stored.length > 0) {
          setWorkspaces((current) => {
            const byId = new Map(current.map((workspace) => [workspace.id, workspace]));
            for (const workspace of stored) {
              const currentWorkspace = byId.get(workspace.id);
              if (!currentWorkspace || workspace.updatedAt >= currentWorkspace.updatedAt) {
                byId.set(workspace.id, workspace);
              }
            }
            return [...byId.values()].sort(
              (left, right) => right.updatedAt - left.updatedAt,
            );
          });
        }
        setWorkspacesHydrated(true);
      })
      .catch(() => {
        if (!disposed) {
          setWorkspacesHydrated(true);
        }
      });
    return () => {
      disposed = true;
    };
  }, []);

  useEffect(() => {
    let disposed = false;
    void Promise.all([loadModelLibraries(), loadLocalModels(), loadModelProfiles()])
      .then(([libraries, models, profiles]) => {
        if (disposed) {
          return;
        }
        setModelLibraries(libraries);
        setLocalModels(models);
        setModelProfiles(profiles);
        setSelectedLocalModelId((current) => current ?? models[0]?.id ?? null);
      })
      .catch(() => {
        // The native model catalog is intentionally unavailable in Vite preview.
      });
    return () => {
      disposed = true;
    };
  }, []);

  useEffect(() => {
    if (!selectedLocalModelId) {
      setLoadEstimate(null);
      return;
    }
    let disposed = false;
    setProfileMessage(null);
    void estimateModelLoad(selectedLocalModelId, loadPreset)
      .then((estimate) => {
        if (!disposed) {
          setLoadEstimate(estimate);
        }
      })
      .catch((estimateError) => {
        if (!disposed) {
          setLoadEstimate(null);
          setProfileMessage(errorMessage(estimateError));
        }
      });
    return () => {
      disposed = true;
    };
  }, [loadPreset, selectedLocalModelId]);

  useEffect(() => {
    const persistenceTimer = window.setTimeout(() => {
      try {
        window.localStorage.setItem(CONVERSATIONS_KEY, JSON.stringify(conversations));
        const nonSecretSettings = {
          base_url: settings.base_url,
          model: settings.model,
          max_tokens: settings.max_tokens,
          temperature: settings.temperature,
          system_prompt: settings.system_prompt,
        };
        window.localStorage.setItem(SETTINGS_KEY, JSON.stringify(nonSecretSettings));
        window.localStorage.setItem(WORKSPACES_KEY, JSON.stringify(workspaces));
        window.localStorage.setItem(UI_PREFERENCES_KEY, JSON.stringify(uiPreferences));
      } catch {
        // Persistence is best effort; the chat remains usable in private browsing.
      }
    }, 300);
    return () => window.clearTimeout(persistenceTimer);
  }, [conversations, settings, uiPreferences, workspaces]);

  useEffect(() => {
    if (!persistenceHydrated || isStreaming) {
      return;
    }
    const persistenceTimer = window.setTimeout(() => {
      for (const conversation of conversations) {
        void savePersistedConversation(toPersistedConversation(conversation)).catch(() => {
          // Vite preview and private browser mode intentionally have no native database.
        });
      }
    }, 450);
    return () => window.clearTimeout(persistenceTimer);
  }, [conversations, isStreaming, persistenceHydrated]);

  useEffect(() => {
    if (!workspacesHydrated) {
      return;
    }
    const persistenceTimer = window.setTimeout(() => {
      for (const workspace of workspaces) {
        void savePersistedWorkspace(workspace).catch(() => {
          // Vite preview and private browser mode intentionally have no native database.
        });
      }
    }, 450);
    return () => window.clearTimeout(persistenceTimer);
  }, [workspaces, workspacesHydrated]);

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
    const history: ChatMessage[] = [];
    if (settings.system_prompt.trim()) {
      history.push({ role: "system", content: settings.system_prompt.trim() });
    }
    history.push(
      ...existingMessages
        .filter((message) => message.content.trim().length > 0)
        .map((message) => ({
          role: message.role,
          content: message.content,
        })),
    );
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
    setProviderDiagnostics(null);
    setModelOptions([]);
    try {
      const diagnostics = await inspectProvider({
        base_url: settings.base_url.trim(),
        model: settings.model.trim() || "default",
        api_key: settings.api_key.trim() || undefined,
      });
      setProviderDiagnostics(diagnostics);
      setModelOptions(diagnostics.models);
      if (!diagnostics.error && !settings.model.trim() && diagnostics.models[0]) {
        setSettings((current) => ({ ...current, model: diagnostics.models[0].id }));
      }
      setConnectionMessage(
        diagnostics.error
          ? diagnostics.error +
            (diagnostics.retryable ? " · " + tx("retryable") : "")
          : diagnostics.models.length === 0
            ? tx("connectedNoModels")
            : tx("modelsAvailable", { count: diagnostics.models.length }),
      );
    } catch (connectionError) {
      setConnectionMessage(errorMessage(connectionError));
    } finally {
      setModelsLoading(false);
    }
  };

  const applyModelScan = (summary: ModelScanSummary) => {
    setModelLibraries((current) => [
      summary.library,
      ...current.filter((library) => library.id !== summary.library.id),
    ]);
    setLocalModels((current) => [
      ...current.filter((model) => model.libraryId !== summary.library.id),
      ...summary.models,
    ]);
    setModelScanSummary(summary);
    setSelectedLocalModelId((current) =>
      current && summary.models.some((model) => model.id === current)
        ? current
        : summary.models[0]?.id ?? null,
    );
  };

  const handleRegisterModelLibrary = async () => {
    const path = modelLibraryPath.trim();
    if (!path) {
      setModelLibraryMessage(tx("modelLibraryPathRequired"));
      return;
    }
    setModelLibraryBusy(true);
    setModelLibraryMessage(null);
    try {
      const diagnostics = await validateWorkspacePath(path);
      setModelLibraryDiagnostics(diagnostics);
      if (!diagnostics.exists || !diagnostics.isDirectory) {
        setModelLibraryMessage(tx("modelLibraryPathInvalid"));
        return;
      }
      const canonicalPath = diagnostics.canonicalPath ?? path;
      const pathParts = canonicalPath.split(/[\\/]/).filter(Boolean);
      const fallbackName = pathParts[pathParts.length - 1] ?? tx("modelLibraryDefaultName");
      const now = Date.now();
      const library: ModelLibrary = {
        id: createId("model-library"),
        name: (modelLibraryName.trim() || fallbackName).slice(0, 128),
        rootPath: canonicalPath,
        enabled: true,
        lastScanAt: null,
        createdAt: now,
        updatedAt: now,
      };
      await saveModelLibrary(library);
      setModelLibraries((current) => [library, ...current]);
      setModelLibraryName("");
      setModelLibraryPath("");
      const summary = await scanModelLibrary(library.id);
      applyModelScan(summary);
      setModelLibraryDiagnostics(null);
      setModelLibraryMessage(
        tx("modelScanComplete", {
          count: summary.scannedCount,
          duration: summary.durationMs,
        }) + (summary.issues.length > 0
          ? " " + tx("modelScanIssues", { count: summary.issues.length })
          : ""),
      );
    } catch (scanError) {
      setModelLibraryMessage(errorMessage(scanError));
    } finally {
      setModelLibraryBusy(false);
    }
  };

  const handleScanModelLibrary = async (libraryId: string) => {
    setModelLibraryBusy(true);
    setModelLibraryMessage(null);
    try {
      const summary = await scanModelLibrary(libraryId);
      applyModelScan(summary);
      setModelLibraryMessage(
        tx("modelScanComplete", {
          count: summary.scannedCount,
          duration: summary.durationMs,
        }) + (summary.issues.length > 0
          ? " " + tx("modelScanIssues", { count: summary.issues.length })
          : ""),
      );
    } catch (scanError) {
      setModelLibraryMessage(errorMessage(scanError));
    } finally {
      setModelLibraryBusy(false);
    }
  };

  const handleDeleteModelLibrary = async (library: ModelLibrary) => {
    if (!window.confirm(tx("deleteModelLibraryConfirm"))) {
      return;
    }
    setModelLibraryBusy(true);
    try {
      await deleteModelLibrary(library.id);
      setModelLibraries((current) => current.filter((item) => item.id !== library.id));
      setLocalModels((current) => current.filter((model) => model.libraryId !== library.id));
      setSelectedLocalModelId((current) => {
        const removed = localModels.find(
          (model) => model.id === current && model.libraryId === library.id,
        );
        return removed ? null : current;
      });
      if (modelScanSummary?.library.id === library.id) {
        setModelScanSummary(null);
      }
    } catch (deleteError) {
      setModelLibraryMessage(errorMessage(deleteError));
    } finally {
      setModelLibraryBusy(false);
    }
  };

  const handleSaveModelProfile = async () => {
    if (!selectedLocalModel || !loadEstimate) {
      return;
    }
    const existing = modelProfiles.find(
      (profile) =>
        profile.modelId === selectedLocalModel.id && profile.preset === loadPreset,
    );
    const now = Date.now();
    const profile: ModelProfile = {
      id: existing?.id ?? createId("model-profile"),
      name: selectedLocalModel.displayName + " · " + loadPreset,
      preset: loadPreset,
      modelId: selectedLocalModel.id,
      configJson: JSON.stringify(loadEstimate.profile),
      createdAt: existing?.createdAt ?? now,
      updatedAt: now,
    };
    try {
      await saveModelProfile(profile);
      setModelProfiles((current) => [
        profile,
        ...current.filter((item) => item.id !== profile.id),
      ]);
      setProfileMessage(tx("profileSaved"));
    } catch (saveError) {
      setProfileMessage(errorMessage(saveError));
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
    void deletePersistedConversation(conversationId).catch(() => {
      // Local storage remains the fallback when the native shell is unavailable.
    });
  };

  const handleRenameConversation = () => {
    if (!activeConversation) {
      return;
    }
    const value = window.prompt(tx("renameConversationPrompt"), activeConversation.title);
    if (value === null) {
      return;
    }
    const title = value.trim().slice(0, 160);
    if (!title) {
      return;
    }
    updateConversation(activeConversation.id, (conversation) => ({
      ...conversation,
      title,
      updatedAt: Date.now(),
    }));
  };

  const handleExportConversation = () => {
    if (!activeConversation || activeConversation.messages.length === 0) {
      return;
    }
    const markdown = [
      "# " + activeConversation.title,
      "",
      ...activeConversation.messages.flatMap((message) => [
        "## " + (message.role === "user" ? tx("you") : tx("appName")),
        "",
        message.content,
        message.reasoning
          ? "\n> " + tx("reasoningTrace") + ": " + message.reasoning.replace(/\n/g, "\n> ")
          : "",
        "",
      ]),
    ].join("\n");
    const blob = new Blob([markdown], { type: "text/markdown;charset=utf-8" });
    const url = URL.createObjectURL(blob);
    const anchor = document.createElement("a");
    anchor.href = url;
    anchor.download = safeExportName(activeConversation.title) + ".md";
    anchor.click();
    window.setTimeout(() => URL.revokeObjectURL(url), 0);
  };

  const handleValidateWorkspace = async () => {
    setWorkspaceBusy(true);
    setWorkspaceMessage(null);
    try {
      const diagnostics = await validateWorkspacePath(workspacePath.trim());
      setWorkspaceDiagnostics(diagnostics);
      setWorkspaceMessage(
        diagnostics.exists && diagnostics.isDirectory
          ? tx("workspacePathReady")
          : tx("workspacePathInvalid"),
      );
    } catch {
      setWorkspaceDiagnostics(null);
      setWorkspaceMessage(tx("coreNotConnected"));
    } finally {
      setWorkspaceBusy(false);
    }
  };

  const handleAddWorkspace = async () => {
    const path = workspacePath.trim();
    if (!path) {
      setWorkspaceMessage(tx("workspacePathRequired"));
      return;
    }
    setWorkspaceBusy(true);
    setWorkspaceMessage(null);
    try {
      const diagnostics = await validateWorkspacePath(path);
      setWorkspaceDiagnostics(diagnostics);
      if (!diagnostics.exists || !diagnostics.isDirectory) {
        setWorkspaceMessage(tx("workspacePathInvalid"));
        return;
      }
      const canonicalPath = diagnostics.canonicalPath ?? path;
      const pathParts = canonicalPath.split(/[\\/]/).filter(Boolean);
      const fallbackName = pathParts[pathParts.length - 1] ?? tx("workspaceDefaultName");
      const now = Date.now();
      const workspace: Workspace = {
        id: createId("workspace"),
        name: (workspaceName.trim() || fallbackName).slice(0, 128),
        rootPath: canonicalPath,
        createdAt: now,
        updatedAt: now,
      };
      setWorkspaces((current) => [
        workspace,
        ...current.filter((item) => item.rootPath !== workspace.rootPath),
      ]);
      void savePersistedWorkspace(workspace).catch(() => {
        // The local fallback remains available in preview mode.
      });
      setWorkspaceName("");
      setWorkspacePath("");
      setWorkspaceDiagnostics(null);
      setWorkspaceMessage(tx("workspaceAdded"));
    } catch {
      setWorkspaceMessage(tx("coreNotConnected"));
    } finally {
      setWorkspaceBusy(false);
    }
  };

  const handleDeleteWorkspace = (workspaceId: string) => {
    if (!window.confirm(tx("deleteWorkspaceConfirm"))) {
      return;
    }
    setWorkspaces((current) => current.filter((workspace) => workspace.id !== workspaceId));
    void deletePersistedWorkspace(workspaceId).catch(() => {
      // Local storage remains the fallback when the native shell is unavailable.
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

  const renderWorkspaces = () => (
    <div className="workspaces-view">
      <section className="settings-panel card workspace-form-card">
        <div className="settings-heading">
          <div>
            <div className="eyebrow">{tx("workspaceRegistry")}</div>
            <h2>{tx("addWorkspaceTitle")}</h2>
          </div>
          <span className="pill pill-green">{tx("scopedAccess")}</span>
        </div>
        <p className="settings-intro">{tx("workspaceIntro")}</p>
        <div className="workspace-form">
          <label>
            <span>{tx("workspaceName")}</span>
            <input
              value={workspaceName}
              onChange={(event) => setWorkspaceName(event.target.value)}
              placeholder={tx("workspaceNamePlaceholder")}
              maxLength={128}
            />
          </label>
          <label className="workspace-path-field">
            <span>{tx("workspacePath")}</span>
            <input
              value={workspacePath}
              onChange={(event) => {
                setWorkspacePath(event.target.value);
                setWorkspaceDiagnostics(null);
                setWorkspaceMessage(null);
              }}
              placeholder={tx("workspacePathPlaceholder")}
              spellCheck={false}
              autoComplete="off"
            />
          </label>
        </div>
        <div className="settings-actions">
          <button
            className="secondary-button"
            type="button"
            onClick={() => void handleValidateWorkspace()}
            disabled={workspaceBusy || !workspacePath.trim()}
          >
            {workspaceBusy ? tx("checking") : tx("validatePath")}
          </button>
          <button
            className="primary-button"
            type="button"
            onClick={() => void handleAddWorkspace()}
            disabled={workspaceBusy || !workspacePath.trim()}
          >
            {tx("addWorkspace")}
          </button>
          {workspaceMessage && (
            <span
              className={
                "connection-message " +
                (workspaceDiagnostics?.exists && workspaceDiagnostics.isDirectory
                  ? "is-success"
                  : "")
              }
              role="status"
            >
              {workspaceMessage}
            </span>
          )}
        </div>
        {workspaceDiagnostics && (
          <div
            className={
              "path-diagnostics " +
              (workspaceDiagnostics.exists && workspaceDiagnostics.isDirectory
                ? "is-valid"
                : "is-invalid")
            }
          >
            <span className="status-dot" />
            <div>
              <strong>
                {workspaceDiagnostics.exists && workspaceDiagnostics.isDirectory
                  ? tx("directoryReady")
                  : tx("directoryUnavailable")}
              </strong>
              <small>
                {workspaceDiagnostics.canonicalPath ??
                  workspaceDiagnostics.error ??
                  tx("directoryUnavailable")}
              </small>
            </div>
          </div>
        )}
        <div className="provider-help workspace-boundary-note">
          <strong>{tx("workspaceSafetyTitle")}</strong>
          <span>{tx("workspaceSafetyDescription")}</span>
        </div>
      </section>

      <section className="card workspace-list-card">
        <div className="card-heading">
          <div>
            <span className="card-title">{tx("registeredWorkspaces")}</span>
            <small className="card-caption">{tx("workspaceCount", { count: workspaces.length })}</small>
          </div>
          <span className="metric-live"><span className="status-dot" /> {tx("storedLocally")}</span>
        </div>
        {workspaces.length === 0 ? (
          <div className="workspace-empty">
            <div className="empty-view-icon" aria-hidden="true">⌁</div>
            <strong>{tx("noWorkspaces")}</strong>
            <p>{tx("noWorkspacesDescription")}</p>
          </div>
        ) : (
          <div className="workspace-list">
            {workspaces.map((workspace) => (
              <article className="workspace-row" key={workspace.id}>
                <div className="workspace-row-icon" aria-hidden="true">⌁</div>
                <div className="workspace-row-copy">
                  <strong>{workspace.name}</strong>
                  <code>{workspace.rootPath}</code>
                  <small>
                    {tx("workspaceAddedAt", {
                      date: new Date(workspace.updatedAt).toLocaleDateString(localeTag),
                    })}
                  </small>
                </div>
                <button
                  className="conversation-delete"
                  type="button"
                  onClick={() => handleDeleteWorkspace(workspace.id)}
                  aria-label={tx("deleteWorkspace")}
                  title={tx("deleteWorkspace")}
                >
                  ×
                </button>
              </article>
            ))}
          </div>
        )}
      </section>
    </div>
  );

  const renderModels = () => (
    <div className="models-view">
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
          <label className="setting-wide">
            <span>{tx("systemPrompt")} <em>{tx("optional")}</em></span>
            <textarea
              value={settings.system_prompt}
              onChange={(event) =>
                setSettings((current) => ({
                  ...current,
                  system_prompt: event.target.value.slice(0, 16_384),
                }))
              }
              placeholder={tx("systemPromptPlaceholder")}
              rows={4}
            />
            <small>{tx("systemPromptHelp")}</small>
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

      {providerDiagnostics && (
        <section
          className={
            "provider-health card " +
            (providerDiagnostics.status === "connected" ? "is-connected" : "is-error")
          }
        >
          <div className="card-heading">
            <div>
              <span className="card-title">{tx("providerHealth")}</span>
              <small className="card-caption">{providerDiagnostics.endpoint}</small>
            </div>
            <span className={"pill " + (providerDiagnostics.status === "connected" ? "pill-green" : "pill-red")}>
              {providerDiagnostics.status === "connected" ? tx("connected") : tx("offline")}
            </span>
          </div>
          <div className="health-stat-grid">
            <div><strong>{Math.round(providerDiagnostics.latencyMs)} ms</strong><span>{tx("diagnosticLatency")}</span></div>
            <div><strong>{providerDiagnostics.modelCount.toLocaleString(localeTag)}</strong><span>{tx("catalogModels")}</span></div>
            <div><strong>{providerDiagnostics.local ? tx("localEndpoint") : tx("remoteEndpoint")}</strong><span>{tx("endpointClass")}</span></div>
          </div>
          {providerDiagnostics.error && (
            <div className="runtime-error">
              {providerDiagnostics.error}
              {providerDiagnostics.retryable && <span> · {tx("retryable")}</span>}
            </div>
          )}
        </section>
      )}

      <section className="model-catalog card">
        <div className="card-heading">
          <div>
            <span className="card-title">{tx("availableModels")}</span>
            <small className="card-caption">
              {selectedProviderModel ? selectedProviderModel.id : tx("checkProviderForModels")}
            </small>
          </div>
          <span className="metric-live"><span className="status-dot" /> {tx("providerCatalog")}</span>
        </div>
        {modelOptions.length === 0 ? (
          <div className="catalog-empty">{tx("noProviderModels")}</div>
        ) : (
          <div className="model-catalog-grid">
            {modelOptions.map((model) => (
              <button
                className={"model-catalog-item " + (settings.model === model.id ? "is-selected" : "")}
                type="button"
                key={model.id}
                onClick={() => setSettings((current) => ({ ...current, model: model.id }))}
              >
                <span className="model-catalog-icon" aria-hidden="true">◈</span>
                <span className="model-catalog-copy">
                  <strong>{model.id}</strong>
                  <small>{model.owned_by ?? tx("providerReported")}</small>
                </span>
                {settings.model === model.id && <span className="model-selected-mark">✓</span>}
              </button>
            ))}
          </div>
        )}
      </section>

      <section className="model-library card">
        <div className="settings-heading">
          <div>
            <div className="eyebrow">{tx("localModelLibrary")}</div>
            <h2>{tx("discoverLocalModels")}</h2>
          </div>
          <span className="pill pill-blue">{tx("metadataOnly")}</span>
        </div>
        <p className="settings-intro">{tx("modelLibraryIntro")}</p>
        <div className="settings-grid">
          <label>
            <span>{tx("modelLibraryName")}</span>
            <input
              value={modelLibraryName}
              onChange={(event) => setModelLibraryName(event.target.value)}
              placeholder={tx("modelLibraryNamePlaceholder")}
              maxLength={128}
            />
          </label>
          <label>
            <span>{tx("modelLibraryPath")}</span>
            <input
              value={modelLibraryPath}
              onChange={(event) => {
                setModelLibraryPath(event.target.value);
                setModelLibraryDiagnostics(null);
                setModelLibraryMessage(null);
              }}
              placeholder={tx("modelLibraryPathPlaceholder")}
              spellCheck={false}
              autoComplete="off"
            />
          </label>
        </div>
        <div className="settings-actions">
          <button
            className="secondary-button"
            type="button"
            onClick={() => void (async () => {
              if (!modelLibraryPath.trim()) {
                setModelLibraryMessage(tx("modelLibraryPathRequired"));
                return;
              }
              try {
                const diagnostics = await validateWorkspacePath(modelLibraryPath.trim());
                setModelLibraryDiagnostics(diagnostics);
                setModelLibraryMessage(
                  diagnostics.exists && diagnostics.isDirectory
                    ? tx("directoryReady")
                    : tx("modelLibraryPathInvalid"),
                );
              } catch (diagnosticError) {
                setModelLibraryMessage(errorMessage(diagnosticError));
              }
            })()}
            disabled={modelLibraryBusy || !modelLibraryPath.trim()}
          >
            {tx("validatePath")}
          </button>
          <button
            className="primary-button"
            type="button"
            onClick={() => void handleRegisterModelLibrary()}
            disabled={modelLibraryBusy || !modelLibraryPath.trim()}
          >
            {modelLibraryBusy ? tx("scanning") : tx("registerAndScan")}
          </button>
          {modelLibraryMessage && (
            <span className="connection-message" role="status">{modelLibraryMessage}</span>
          )}
        </div>
        {modelLibraryDiagnostics && (
          <div
            className={
              "path-diagnostics " +
              (modelLibraryDiagnostics.exists && modelLibraryDiagnostics.isDirectory
                ? "is-valid"
                : "is-invalid")
            }
          >
            <span className="status-dot" />
            <div>
              <strong>
                {modelLibraryDiagnostics.exists && modelLibraryDiagnostics.isDirectory
                  ? tx("directoryReady")
                  : tx("directoryUnavailable")}
              </strong>
              <small>
                {modelLibraryDiagnostics.canonicalPath ??
                  modelLibraryDiagnostics.error ??
                  tx("directoryUnavailable")}
              </small>
            </div>
          </div>
        )}
        <div className="provider-help">
          <strong>{tx("scannerSafetyTitle")}</strong>
          <span>{tx("scannerSafetyDescription")}</span>
        </div>
      </section>

      <section className="card model-library-list-card">
        <div className="card-heading">
          <div>
            <span className="card-title">{tx("registeredModelLibraries")}</span>
            <small className="card-caption">
              {tx("modelLibraryCount", { count: modelLibraries.length })}
            </small>
          </div>
          <span className="metric-live"><span className="status-dot" /> {tx("storedLocally")}</span>
        </div>
        {modelLibraries.length === 0 ? (
          <div className="catalog-empty">
            <strong>{tx("noModelLibraries")}</strong>
            <p>{tx("noModelLibrariesDescription")}</p>
          </div>
        ) : (
          <div className="model-library-list">
            {modelLibraries.map((library) => (
              <article className="model-library-row" key={library.id}>
                <div className="workspace-row-icon" aria-hidden="true">◈</div>
                <div className="workspace-row-copy">
                  <strong>{library.name}</strong>
                  <code>{library.rootPath}</code>
                  <small>
                    {library.lastScanAt
                      ? tx("lastScanned", {
                          date: new Date(library.lastScanAt).toLocaleString(localeTag),
                        })
                      : tx("neverScanned")}
                  </small>
                </div>
                <div className="model-library-row-actions">
                  <button
                    className="secondary-button compact-button"
                    type="button"
                    onClick={() => void handleScanModelLibrary(library.id)}
                    disabled={modelLibraryBusy}
                  >
                    {tx("scanLibrary")}
                  </button>
                  <button
                    className="conversation-delete"
                    type="button"
                    onClick={() => void handleDeleteModelLibrary(library)}
                    disabled={modelLibraryBusy}
                    aria-label={tx("deleteModelLibrary")}
                    title={tx("deleteModelLibrary")}
                  >
                    ×
                  </button>
                </div>
              </article>
            ))}
          </div>
        )}
      </section>

      <section className="card local-inventory-card">
        <div className="card-heading">
          <div>
            <span className="card-title">{tx("localModelInventory")}</span>
            <small className="card-caption">
              {tx("localModelCount", { count: localModels.length })}
              {modelScanSummary
                ? " · " + tx("visitedFiles", { count: modelScanSummary.visitedFiles })
                : ""}
            </small>
          </div>
          <span className="metric-live"><span className="status-dot" /> {tx("metadataOnly")}</span>
        </div>
        {localModels.length === 0 ? (
          <div className="catalog-empty">
            <strong>{tx("noLocalModels")}</strong>
            <p>{tx("noLocalModelsDescription")}</p>
          </div>
        ) : (
          <div className="local-model-grid">
            {localModels.map((model) => (
              <article
                className={"local-model-card " + (selectedLocalModelId === model.id ? "is-selected" : "")}
                key={model.id}
              >
                <button
                  className="local-model-select"
                  type="button"
                  onClick={() => {
                    setSelectedLocalModelId(model.id);
                    setLoadEstimate(null);
                    setProfileMessage(null);
                  }}
                >
                  <span className="model-catalog-icon" aria-hidden="true">◈</span>
                  <span className="local-model-title">
                    <strong>{model.displayName}</strong>
                    <small>{model.quantization ?? model.format.toUpperCase()}</small>
                  </span>
                  {selectedLocalModelId === model.id && (
                    <span className="model-selected-mark">✓</span>
                  )}
                </button>
                <div className="local-model-stats">
                  <span>{tx("modelParameters", {
                    count: model.parameterCount === null
                      ? tx("unknownValue")
                      : formatParameterCount(model.parameterCount, localeTag),
                  })}</span>
                  <span>{formatBytes(model.fileSizeBytes)}</span>
                  <span>{model.architecture ?? "—"}</span>
                  <span>
                    {model.contextCapacity
                      ? tx("modelContextValue", { count: model.contextCapacity })
                      : tx("unknownValue")}
                  </span>
                </div>
                <code className="local-model-path">{model.filePath}</code>
                <div className="capability-row">
                  {model.vision && <span>{tx("capabilityVision")}</span>}
                  {model.toolCalling && <span>{tx("capabilityTools")}</span>}
                  {model.reasoning && <span>{tx("capabilityReasoning")}</span>}
                  {model.embeddings && <span>{tx("capabilityEmbeddings")}</span>}
                  {!model.vision && !model.toolCalling && !model.reasoning && !model.embeddings && (
                    <span className="muted">{tx("noCapabilities")}</span>
                  )}
                </div>
              </article>
            ))}
          </div>
        )}
        {modelScanSummary && modelScanSummary.issues.length > 0 && (
          <details className="model-scan-issues">
            <summary>
              {tx("modelScanIssues", { count: modelScanSummary.issues.length })}
            </summary>
            <ul>
              {modelScanSummary.issues.slice(0, 12).map((issue) => (
                <li key={issue.path + issue.message}>
                  <code>{issue.path}</code>
                  <span>{issue.message}</span>
                </li>
              ))}
            </ul>
          </details>
        )}
      </section>

      {selectedLocalModel && (
        <section className="card load-profile-card">
          <div className="card-heading">
            <div>
              <span className="card-title">{tx("loadProfile")}</span>
              <small className="card-caption">{selectedLocalModel.displayName}</small>
            </div>
            <span className="pill pill-blue">{tx("preflightValidated")}</span>
          </div>
          <p className="settings-intro">{tx("loadProfileDescription")}</p>
          <div className="profile-controls">
            <label>
              <span>{tx("profilePreset")}</span>
              <select
                value={loadPreset}
                onChange={(event) => {
                  setLoadPreset(event.target.value as LoadPreset);
                  setProfileMessage(null);
                }}
              >
                <option value="eco">{tx("profileEco")}</option>
                <option value="balanced">{tx("profileBalanced")}</option>
                <option value="performance">{tx("profilePerformance")}</option>
              </select>
            </label>
            <button
              className="secondary-button"
              type="button"
              onClick={() => void handleSaveModelProfile()}
              disabled={!loadEstimate}
            >
              {tx("saveProfile")}
            </button>
            {profileMessage && <span className="connection-message" role="status">{profileMessage}</span>}
          </div>
          {loadEstimate ? (
            <>
              <div className="estimate-stat-grid">
                <div><strong>{formatBytes(loadEstimate.estimate.weights_bytes)}</strong><span>{tx("estimateWeights")}</span></div>
                <div><strong>{formatBytes(loadEstimate.estimate.kv_cache_bytes)}</strong><span>{tx("estimateKvCache")}</span></div>
                <div><strong>{formatBytes(loadEstimate.estimate.estimated_vram_bytes)}</strong><span>{tx("estimateVram")}</span></div>
                <div><strong>{formatBytes(loadEstimate.estimate.estimated_ram_bytes)}</strong><span>{tx("estimateRam")}</span></div>
              </div>
              <div className="estimate-assumptions">
                <strong>{tx("estimateConfidence")}: {loadEstimate.estimate.confidence.toUpperCase()}</strong>
                <ul>
                  {loadEstimate.estimate.assumptions.map((assumption) => <li key={assumption}>{assumption}</li>)}
                </ul>
              </div>
            </>
          ) : (
            <div className="catalog-empty">
              {profileMessage ?? tx("estimateLoading")}
            </div>
          )}
        </section>
      )}
    </div>
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
              {view === "chats" && activeConversation && (
                <>
                  <button
                    className="icon-button"
                    type="button"
                    aria-label={tx("renameConversation")}
                    title={tx("renameConversation")}
                    onClick={handleRenameConversation}
                  >
                    ✎
                  </button>
                  <button
                    className="icon-button"
                    type="button"
                    aria-label={tx("exportConversation")}
                    title={tx("exportConversation")}
                    onClick={handleExportConversation}
                    disabled={activeMessages.length === 0}
                  >
                    ⇩
                  </button>
                </>
              )}
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
          {view === "workspaces" && renderWorkspaces()}
          {view === "models" && renderModels()}
          {view === "automations" && renderRoadmap()}

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
            {providerDiagnostics && (
              <div className={"runtime-provider-status " + (providerDiagnostics.status === "connected" ? "is-connected" : "is-error")}>
                <span className="status-dot" />
                <span>
                  {providerDiagnostics.status === "connected"
                    ? tx("providerConnected")
                    : tx("providerUnreachable")}
                </span>
                <small>{Math.round(providerDiagnostics.latencyMs)} ms</small>
              </div>
            )}
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
        <span>{persistenceHydrated ? tx("databaseReady") : tx("databaseLoading")}</span>
        <span className="statusbar-spacer" />
        <span>{tx("apiKeySessionOnly")}</span>
      </footer>
    </div>
  );
}

export default App;
