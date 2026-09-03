import { invoke } from "@tauri-apps/api/core";
import { RuntimeStatusSchema, type RuntimeStatus } from "./protocol";

export async function getRuntimeStatus(): Promise<RuntimeStatus> {
  const raw = await invoke<unknown>("runtime_status");
  return RuntimeStatusSchema.parse(raw);
}

export async function stopEverything(): Promise<number> {
  return invoke<number>("stop_everything");
}

