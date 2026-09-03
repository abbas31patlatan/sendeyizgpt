import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import {
  ConversationSchema,
  GenerationEventSchema,
  MessageSchema,
  RuntimeAvailabilitySchema,
  RuntimeSnapshotSchema,
  RuntimeStatusSchema,
  type Conversation,
  type GenerationEvent,
  type LoadPreset,
  type Message,
  type RuntimeAvailability,
  type RuntimeSnapshot,
  type RuntimeStatus,
} from "./protocol";

export async function getRuntimeStatus(): Promise<RuntimeStatus> {
  return RuntimeStatusSchema.parse(await invoke<unknown>("runtime_status"));
}

export async function getRuntimeSnapshot(): Promise<RuntimeSnapshot> {
  return RuntimeSnapshotSchema.parse(await invoke<unknown>("runtime_snapshot"));
}

export async function getRuntimeAvailability(): Promise<RuntimeAvailability> {
  return RuntimeAvailabilitySchema.parse(await invoke<unknown>("runtime_availability"));
}

export async function loadLocalModel(input: {
  modelPath: string;
  preset: LoadPreset;
  contextLength: number;
  cpuThreads: number;
  gpuOffloadPercent: number;
}): Promise<RuntimeSnapshot> {
  const raw = await invoke<unknown>("load_local_model", {
    request: {
      model_path: input.modelPath,
      preset: input.preset,
      context_length: input.contextLength,
      cpu_threads: input.cpuThreads,
      gpu_offload_percent: input.gpuOffloadPercent,
    },
  });
  return RuntimeSnapshotSchema.parse(raw);
}

export async function unloadLocalModel(): Promise<void> {
  await invoke("unload_local_model");
}

export async function createConversation(title: string): Promise<Conversation> {
  return ConversationSchema.parse(await invoke<unknown>("create_conversation", { title }));
}

export async function listConversations(): Promise<Conversation[]> {
  return ConversationSchema.array().parse(await invoke<unknown>("list_conversations"));
}

export async function listMessages(conversationId: string): Promise<Message[]> {
  return MessageSchema.array().parse(
    await invoke<unknown>("list_messages", { conversationId }),
  );
}

export async function startGeneration(input: {
  conversationId: string;
  messages: Array<{ role: string; content: string }>;
}): Promise<string> {
  return invoke<string>("start_generation", {
    request: {
      conversation_id: input.conversationId,
      messages: input.messages,
      max_tokens: 1024,
      temperature: 0.7,
      top_p: 0.9,
    },
  });
}

export async function onGenerationEvent(
  handler: (event: GenerationEvent) => void,
): Promise<UnlistenFn> {
  return listen<unknown>("generation-event", (event) => {
    const parsed = GenerationEventSchema.safeParse(event.payload);
    if (parsed.success) handler(parsed.data);
  });
}

export async function stopEverything(): Promise<number> {
  return invoke<number>("stop_everything");
}
