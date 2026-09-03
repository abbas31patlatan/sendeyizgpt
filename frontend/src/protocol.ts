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

export function formatBytes(bytes: number | null): string {
  if (bytes === null) {
    return "—";
  }
  const units = ["B", "KB", "MB", "GB", "TB"];
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${value.toFixed(unit === 0 ? 0 : 1)} ${units[unit]}`;
}
