import { z } from "zod";

export const RuntimeStatusSchema = z.object({
  app_version: z.string(),
  core_state: z.enum(["starting", "ready", "degraded", "stopped"]),
  model_name: z.string().nullable(),
  backend_name: z.string().nullable(),
  accelerator: z.string().nullable(),
  gpu_name: z.string().nullable(),
  vram_bytes: z.number().int().nonnegative().nullable(),
  context_length: z.number().int().positive().nullable(),
  tokens_per_second: z.number().nonnegative().nullable(),
  last_error: z.string().nullable(),
});

export const RuntimeSnapshotSchema = z.object({
  running: z.boolean(),
  model_path: z.string().nullable(),
  model_name: z.string().nullable(),
  profile: z.enum(["eco", "balanced", "performance", "custom"]).nullable(),
  context_length: z.number().int().positive().nullable(),
  accelerator: z.string().nullable(),
  port: z.number().int().positive().nullable(),
  tokens_per_second: z.number().nonnegative().nullable(),
  last_error: z.string().nullable(),
});

export const RuntimeAvailabilitySchema = z.object({
  available: z.boolean(),
  executable_path: z.string(),
  source: z.literal("bundled_llama_cpp_vulkan"),
});

export const ConversationSchema = z.object({
  id: z.string().uuid(),
  title: z.string(),
  pinned: z.boolean(),
  created_at: z.string(),
  updated_at: z.string(),
});

export const MessageSchema = z.object({
  id: z.string().uuid(),
  conversation_id: z.string().uuid(),
  role: z.enum(["system", "developer", "user", "assistant", "tool"]),
  content: z.string(),
  created_at: z.string(),
});

const CompletionSummarySchema = z.object({
  prompt_tokens: z.number().int().nonnegative().nullable(),
  generated_tokens: z.number().int().nonnegative().nullable(),
  elapsed_ms: z.number().int().nonnegative(),
  tokens_per_second: z.number().nonnegative().nullable(),
});

export const GenerationEventSchema = z.discriminatedUnion("type", [
  z.object({ type: z.literal("started"), operation_id: z.string().uuid() }),
  z.object({
    type: z.literal("delta"),
    operation_id: z.string().uuid(),
    text: z.string(),
  }),
  z.object({
    type: z.literal("finished"),
    operation_id: z.string().uuid(),
    message: MessageSchema,
    summary: CompletionSummarySchema,
  }),
  z.object({
    type: z.literal("failed"),
    operation_id: z.string().uuid(),
    message: z.string(),
  }),
]);

export type RuntimeStatus = z.infer<typeof RuntimeStatusSchema>;
export type RuntimeSnapshot = z.infer<typeof RuntimeSnapshotSchema>;
export type RuntimeAvailability = z.infer<typeof RuntimeAvailabilitySchema>;
export type Conversation = z.infer<typeof ConversationSchema>;
export type Message = z.infer<typeof MessageSchema>;
export type GenerationEvent = z.infer<typeof GenerationEventSchema>;
export type LoadPreset = "eco" | "balanced" | "performance";

export const initialUnavailableStatus: RuntimeStatus = {
  app_version: "—",
  core_state: "degraded",
  model_name: null,
  backend_name: null,
  accelerator: null,
  gpu_name: null,
  vram_bytes: null,
  context_length: null,
  tokens_per_second: null,
  last_error: "Desktop core is not connected. Launch the Tauri application.",
};

export const initialRuntimeSnapshot: RuntimeSnapshot = {
  running: false,
  model_path: null,
  model_name: null,
  profile: null,
  context_length: null,
  accelerator: null,
  port: null,
  tokens_per_second: null,
  last_error: null,
};

export function formatBytes(bytes: number | null): string {
  if (bytes === null) return "—";
  const units = ["B", "KB", "MB", "GB", "TB"];
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${value.toFixed(unit === 0 ? 0 : 1)} ${units[unit]}`;
}
