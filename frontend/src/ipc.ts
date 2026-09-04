import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import {
  ChatEventSchema,
  parseOperationStarted,
  ProviderModelSchema,
  RuntimeStatusSchema,
  type ChatEvent,
  type ChatRequest,
  type ProviderConfig,
  type ProviderModel,
  type RuntimeStatus,
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
