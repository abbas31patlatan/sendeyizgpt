import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import {
  ChatEventSchema,
  parseOperationStarted,
  PersistedConversationSchema,
  PersistedWorkspaceSchema,
  ProviderDiagnosticsSchema,
  ProviderModelSchema,
  RuntimeStatusSchema,
  WorkspacePathDiagnosticsSchema,
  type ChatEvent,
  type ChatRequest,
  type PersistedConversation,
  type PersistedWorkspace,
  type ProviderConfig,
  type ProviderDiagnostics,
  type ProviderModel,
  type RuntimeStatus,
  type WorkspacePathDiagnostics,
} from "./protocol";

export async function getRuntimeStatus(): Promise<RuntimeStatus> {
  const raw = await invoke<unknown>("runtime_status");
  return RuntimeStatusSchema.parse(raw);
}

export async function stopEverything(): Promise<number> {
  return invoke<number>("stop_everything");
}

export async function cancelOperation(operationId: string): Promise<boolean> {
  return invoke<boolean>("cancel_operation", { operationId });
}

export async function startChat(request: ChatRequest): Promise<{ operation_id: string }> {
  const raw = await invoke<unknown>("start_chat", { request });
  return parseOperationStarted(raw);
}

export async function listProviderModels(config: ProviderConfig): Promise<ProviderModel[]> {
  const raw = await invoke<unknown>("list_provider_models", {
    config: {
      ...config,
      api_key: config.api_key?.trim() || undefined,
    },
  });
  return ProviderModelSchema.array().parse(raw);
}

export async function inspectProvider(config: ProviderConfig): Promise<ProviderDiagnostics> {
  const raw = await invoke<unknown>("inspect_provider", {
    config: {
      ...config,
      api_key: config.api_key?.trim() || undefined,
    },
  });
  return ProviderDiagnosticsSchema.parse(raw);
}

export async function loadPersistedConversations(): Promise<PersistedConversation[]> {
  const raw = await invoke<unknown>("load_conversations");
  return PersistedConversationSchema.array().parse(raw);
}

export async function savePersistedConversation(
  conversation: PersistedConversation,
): Promise<void> {
  await invoke("save_conversation", { conversation });
}

export async function deletePersistedConversation(conversationId: string): Promise<boolean> {
  return invoke<boolean>("delete_conversation", { conversationId });
}

export async function loadPersistedWorkspaces(): Promise<PersistedWorkspace[]> {
  const raw = await invoke<unknown>("load_workspaces");
  return PersistedWorkspaceSchema.array().parse(raw);
}

export async function savePersistedWorkspace(workspace: PersistedWorkspace): Promise<void> {
  await invoke("save_workspace", { workspace });
}

export async function deletePersistedWorkspace(workspaceId: string): Promise<boolean> {
  return invoke<boolean>("delete_workspace", { workspaceId });
}

export async function validateWorkspacePath(
  path: string,
): Promise<WorkspacePathDiagnostics> {
  const raw = await invoke<unknown>("validate_workspace_path", { path });
  return WorkspacePathDiagnosticsSchema.parse(raw);
}

export async function listenChatEvents(
  onEvent: (event: ChatEvent) => void,
): Promise<UnlistenFn> {
  return listen<unknown>("aegis://chat", (event) => {
    const parsed = ChatEventSchema.safeParse(event.payload);
    if (parsed.success) {
      onEvent(parsed.data);
    }
  });
}
