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

export type RuntimeStatus = z.infer<typeof RuntimeStatusSchema>;

export const NativeRuntimePhaseSchema = z.enum([
  "stopped",
  "starting",
  "loading",
  "ready",
  "stopping",
  "error",
]);

export type NativeRuntimePhase = z.infer<typeof NativeRuntimePhaseSchema>;

export const NativeRuntimeMetricsSchema = z.object({
  promptTokensTotal: z.number().int().nonnegative().nullable(),
  promptSecondsTotal: z.number().nonnegative().nullable(),
  promptTokensPerSecond: z.number().nonnegative().nullable(),
  predictedTokensTotal: z.number().int().nonnegative().nullable(),
  predictedSecondsTotal: z.number().nonnegative().nullable(),
  predictedTokensPerSecond: z.number().nonnegative().nullable(),
  requestsProcessing: z.number().int().nonnegative().nullable(),
  requestsDeferred: z.number().int().nonnegative().nullable(),
  contextTokensMax: z.number().int().nonnegative().nullable(),
});

export type NativeRuntimeMetrics = z.infer<typeof NativeRuntimeMetricsSchema>;

export const NativeRuntimeStatusSchema = z.object({
  phase: NativeRuntimePhaseSchema,
  modelId: z.string().nullable(),
  modelName: z.string().nullable(),
  executablePath: z.string().nullable(),
  endpoint: z.string().nullable(),
  processId: z.number().int().positive().nullable(),
  startedAtUnixMs: z.number().int().nonnegative().nullable(),
  contextLength: z.number().int().positive().nullable(),
  gpuOffloadPercent: z.number().int().min(0).max(100).nullable(),
  message: z.string().nullable(),
  metrics: NativeRuntimeMetricsSchema.nullable(),
});

export type NativeRuntimeStatus = z.infer<typeof NativeRuntimeStatusSchema>;

export const ChatRoleSchema = z.enum([
  "system",
  "developer",
  "user",
  "assistant",
  "tool",
]);

export const ChatMessageSchema = z.object({
  role: ChatRoleSchema,
  content: z.string(),
});

export type ChatRole = z.infer<typeof ChatRoleSchema>;
export type ChatMessage = z.infer<typeof ChatMessageSchema>;

export const ProviderConfigSchema = z.object({
  base_url: z.string().min(1),
  model: z.string().min(1),
  api_key: z.string().optional(),
});

export type ProviderConfig = z.infer<typeof ProviderConfigSchema>;

export const ChatRequestSchema = z.object({
  provider: ProviderConfigSchema,
  messages: z.array(ChatMessageSchema).min(1),
  max_tokens: z.number().int().positive(),
  temperature: z.number().min(0).max(2),
});

export type ChatRequest = z.infer<typeof ChatRequestSchema>;

export const ProviderModelSchema = z.object({
  id: z.string().min(1),
  owned_by: z.string().nullable(),
});

export type ProviderModel = z.infer<typeof ProviderModelSchema>;

export const ProviderDiagnosticsSchema = z.object({
  status: z.enum(["connected", "error"]),
  endpoint: z.string().min(1),
  local: z.boolean(),
  latencyMs: z.number().nonnegative(),
  modelCount: z.number().int().nonnegative(),
  models: ProviderModelSchema.array(),
  error: z.string().nullable(),
  retryable: z.boolean(),
});

export type ProviderDiagnostics = z.infer<typeof ProviderDiagnosticsSchema>;


export const ModelLibrarySchema = z.object({
  id: z.string().min(1),
  name: z.string().min(1),
  rootPath: z.string().min(1),
  enabled: z.boolean(),
  lastScanAt: z.number().int().nonnegative().nullable(),
  createdAt: z.number().int().nonnegative(),
  updatedAt: z.number().int().nonnegative(),
});

export type ModelLibrary = z.infer<typeof ModelLibrarySchema>;

export const LocalModelSchema = z.object({
  id: z.string().min(1),
  libraryId: z.string().min(1),
  displayName: z.string().min(1),
  filePath: z.string().min(1),
  format: z.enum(["gguf", "safetensors", "unknown"]),
  family: z.string().nullable(),
  parameterCount: z.number().int().nonnegative().nullable(),
  architecture: z.string().nullable(),
  quantization: z.string().nullable(),
  ggufVersion: z.string().nullable(),
  fileSizeBytes: z.number().int().nonnegative(),
  contextCapacity: z.number().int().nonnegative().nullable(),
  vision: z.boolean(),
  toolCalling: z.boolean(),
  reasoning: z.boolean(),
  embeddings: z.boolean(),
  metadataHash: z.string().nullable(),
  lastSeenAt: z.number().int().nonnegative(),
});

export type LocalModel = z.infer<typeof LocalModelSchema>;

export const ModelScanIssueSchema = z.object({
  path: z.string().min(1),
  message: z.string().min(1),
});

export type ModelScanIssue = z.infer<typeof ModelScanIssueSchema>;

export const ModelScanSummarySchema = z.object({
  library: ModelLibrarySchema,
  models: LocalModelSchema.array(),
  scannedCount: z.number().int().nonnegative(),
  visitedFiles: z.number().int().nonnegative(),
  issues: ModelScanIssueSchema.array(),
  durationMs: z.number().int().nonnegative(),
});

export type ModelScanSummary = z.infer<typeof ModelScanSummarySchema>;

export const LoadPresetSchema = z.enum(["eco", "balanced", "performance", "custom"]);
export type LoadPreset = z.infer<typeof LoadPresetSchema>;

export const LoadProfileSchema = z.object({
  preset: LoadPresetSchema,
  context_length: z.number().int().positive(),
  gpu_offload_percent: z.number().int().min(0).max(100),
  cpu_threads: z.number().int().positive(),
  batch_size: z.number().int().positive(),
  physical_batch_size: z.number().int().positive(),
  flash_attention: z.boolean(),
  kv_cache_offload: z.boolean(),
  k_cache_quantization: z.enum(["f32", "f16", "q8", "q6", "q4"]),
  v_cache_quantization: z.enum(["f32", "f16", "q8", "q6", "q4"]),
  mmap: z.boolean(),
  mlock: z.boolean(),
  parallel_requests: z.number().int().positive(),
  reasoning_enabled: z.boolean(),
  reasoning_budget_tokens: z.number().int().nonnegative().nullable(),
  temperature: z.number().min(0).max(2),
  top_p: z.number().min(0).max(1),
  top_k: z.number().int().nonnegative(),
  min_p: z.number().min(0).max(1),
  repeat_penalty: z.number().positive(),
  seed: z.number().int().nonnegative().nullable(),
});

export type LoadProfile = z.infer<typeof LoadProfileSchema>;

export const MemoryEstimateSchema = z.object({
  weights_bytes: z.number().int().nonnegative(),
  kv_cache_bytes: z.number().int().nonnegative(),
  scratch_bytes: z.number().int().nonnegative(),
  estimated_vram_bytes: z.number().int().nonnegative(),
  estimated_ram_bytes: z.number().int().nonnegative(),
  confidence: z.enum(["low", "medium", "high"]),
  assumptions: z.string().array(),
});

export type MemoryEstimate = z.infer<typeof MemoryEstimateSchema>;

export const ModelLoadEstimateSchema = z.object({
  model: LocalModelSchema,
  profile: LoadProfileSchema,
  estimate: MemoryEstimateSchema,
});

export type ModelLoadEstimate = z.infer<typeof ModelLoadEstimateSchema>;

export const ModelProfileSchema = z.object({
  id: z.string().min(1),
  name: z.string().min(1),
  preset: LoadPresetSchema,
  modelId: z.string().min(1).nullable(),
  configJson: z.string().min(2),
  createdAt: z.number().int().nonnegative(),
  updatedAt: z.number().int().nonnegative(),
});

export type ModelProfile = z.infer<typeof ModelProfileSchema>;

export const AutomationStatusSchema = z.enum([
  "idle",
  "running",
  "success",
  "error",
  "cancelled",
]);

export type AutomationStatus = z.infer<typeof AutomationStatusSchema>;

export const AutomationSchema = z.object({
  id: z.string().min(1),
  name: z.string().min(1),
  prompt: z.string().min(1),
  intervalMinutes: z.number().int().min(1).max(10080),
  enabled: z.boolean(),
  lastRunAt: z.number().int().nonnegative().nullable(),
  nextRunAt: z.number().int().nonnegative().nullable(),
  lastStatus: AutomationStatusSchema,
  lastError: z.string().nullable(),
  lastConversationId: z.string().nullable(),
  createdAt: z.number().int().nonnegative(),
  updatedAt: z.number().int().nonnegative(),
});

export type Automation = z.infer<typeof AutomationSchema>;

export const PersistedMessageSchema = z.object({
  id: z.string().min(1),
  role: ChatRoleSchema,
  content: z.string(),
  reasoning: z.string().nullable().optional(),
  createdAt: z.number().int().nonnegative(),
});

export type PersistedMessage = z.infer<typeof PersistedMessageSchema>;

export const PersistedConversationSchema = z.object({
  id: z.string().min(1),
  title: z.string().min(1),
  updatedAt: z.number().int().nonnegative(),
  messages: PersistedMessageSchema.array(),
});

export type PersistedConversation = z.infer<typeof PersistedConversationSchema>;

export const PersistedWorkspaceSchema = z.object({
  id: z.string().min(1),
  name: z.string().min(1),
  rootPath: z.string().min(1),
  createdAt: z.number().int().nonnegative(),
  updatedAt: z.number().int().nonnegative(),
});

export type PersistedWorkspace = z.infer<typeof PersistedWorkspaceSchema>;

export const WorkspacePathDiagnosticsSchema = z.object({
  exists: z.boolean(),
  isDirectory: z.boolean(),
  canonicalPath: z.string().nullable(),
  error: z.string().nullable(),
});

export type WorkspacePathDiagnostics = z.infer<typeof WorkspacePathDiagnosticsSchema>;

const OperationStartedSchema = z.object({
  operation_id: z.string().min(1),
});

export const ChatEventSchema = z.discriminatedUnion("type", [
  z.object({
    type: z.literal("started"),
    operation_id: z.string().min(1),
  }),
  z.object({
    type: z.literal("token"),
    operation_id: z.string().min(1),
    text: z.string(),
  }),
  z.object({
    type: z.literal("reasoning"),
    operation_id: z.string().min(1),
    text: z.string(),
  }),
  z.object({
    type: z.literal("finished"),
    operation_id: z.string().min(1),
    generated_tokens: z.number().int().nonnegative(),
    prompt_tokens: z.number().int().nonnegative().nullable(),
    time_to_first_token_ms: z.number().nonnegative().nullable(),
    generation_duration_ms: z.number().nonnegative(),
    finish_reason: z.string().nullable(),
  }),
  z.object({
    type: z.literal("failed"),
    operation_id: z.string().min(1),
    code: z.string(),
    message: z.string(),
    retryable: z.boolean(),
  }),
  z.object({
    type: z.literal("cancelled"),
    operation_id: z.string().min(1),
  }),
]);

export type ChatEvent = z.infer<typeof ChatEventSchema>;
export type OperationStarted = z.infer<typeof OperationStartedSchema>;

export function parseOperationStarted(value: unknown): OperationStarted {
  return OperationStartedSchema.parse(value);
}


export const initialNativeRuntimeStatus: NativeRuntimeStatus = {
  phase: "stopped",
  modelId: null,
  modelName: null,
  executablePath: null,
  endpoint: null,
  processId: null,
  startedAtUnixMs: null,
  contextLength: null,
  gpuOffloadPercent: null,
  message: null,
  metrics: null,
};

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
  last_error: "Desktop core is not connected. Launch the Tauri shell to connect.",
};

export function formatBytes(bytes: number | null | undefined): string {
  if (bytes === null || bytes === undefined) {
    return "—";
  }
  const units = ["B", "KB", "MB", "GB", "TB"];
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return value.toFixed(unit === 0 ? 0 : 1) + " " + units[unit];
}
