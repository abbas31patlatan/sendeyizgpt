use super::{
    CacheQuantization, InferenceError, LoadProfile, ModelDescriptor, ModelFormat,
};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use std::ffi::OsString;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::time::sleep;

const STARTUP_TIMEOUT: Duration = Duration::from_secs(300);
const HEALTH_POLL_INTERVAL: Duration = Duration::from_millis(250);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeRuntimePhase {
    Stopped,
    Starting,
    Loading,
    Ready,
    Stopping,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeRuntimeStatus {
    pub phase: NativeRuntimePhase,
    pub model_id: Option<String>,
    pub model_name: Option<String>,
    pub executable_path: Option<String>,
    pub endpoint: Option<String>,
    pub process_id: Option<u32>,
    pub started_at_unix_ms: Option<i64>,
    pub context_length: Option<u32>,
    pub gpu_offload_percent: Option<u8>,
    pub message: Option<String>,
}

impl NativeRuntimeStatus {
    pub fn stopped() -> Self {
        Self {
            phase: NativeRuntimePhase::Stopped,
            model_id: None,
            model_name: None,
            executable_path: None,
            endpoint: None,
            process_id: None,
            started_at_unix_ms: None,
            context_length: None,
            gpu_offload_percent: None,
            message: None,
        }
    }
}

struct RuntimeState {
    generation: u64,
    child: Option<Child>,
    status: NativeRuntimeStatus,
}

pub struct LlamaServerRuntime {
    state: Mutex<RuntimeState>,
    http: reqwest::Client,
}

impl LlamaServerRuntime {
    pub fn new() -> Result<Self, InferenceError> {
        let http = reqwest::Client::builder()
            .user_agent("Aegis-AI/0.1 llama.cpp supervisor")
            .connect_timeout(Duration::from_millis(500))
            .timeout(Duration::from_secs(3))
            .build()
            .map_err(|error| InferenceError::Backend(format!("native runtime HTTP client: {error}")))?;

        Ok(Self {
            state: Mutex::new(RuntimeState {
                generation: 0,
                child: None,
                status: NativeRuntimeStatus::stopped(),
            }),
            http,
        })
    }

    pub fn status(&self) -> Result<NativeRuntimeStatus, InferenceError> {
        let mut state = self.lock_state()?;
        Self::refresh_locked(&mut state)?;
        Ok(state.status.clone())
    }

    pub async fn start(
        &self,
        executable_path: PathBuf,
        model: ModelDescriptor,
        profile: LoadProfile,
    ) -> Result<NativeRuntimeStatus, InferenceError> {
        validate_native_model(&model)?;
        profile.validate_for_model(&model)?;
        let executable = validate_executable(&executable_path)?;

        let listener = TcpListener::bind(("127.0.0.1", 0)).map_err(|error| {
            InferenceError::WorkerUnavailable(format!(
                "could not reserve a loopback port for llama.cpp: {error}"
            ))
        })?;
        let port = listener
            .local_addr()
            .map_err(|error| {
                InferenceError::WorkerUnavailable(format!(
                    "could not inspect the loopback port for llama.cpp: {error}"
                ))
            })?
            .port();
        drop(listener);

        let endpoint = format!("http://127.0.0.1:{port}");
        let generation = {
            let mut state = self.lock_state()?;
            Self::refresh_locked(&mut state)?;
            if state.child.is_some() {
                return Err(InferenceError::WorkerUnavailable(
                    "another native model is already loaded; unload it first".to_owned(),
                ));
            }

            state.generation = state.generation.wrapping_add(1);
            let generation = state.generation;
            state.status = NativeRuntimeStatus {
                phase: NativeRuntimePhase::Starting,
                model_id: Some(model.id.clone()),
                model_name: Some(model.display_name.clone()),
                executable_path: Some(executable.to_string_lossy().into_owned()),
                endpoint: Some(endpoint.clone()),
                process_id: None,
                started_at_unix_ms: Some(unix_now_millis()),
                context_length: Some(profile.context_length),
                gpu_offload_percent: Some(profile.gpu_offload_percent),
                message: Some("starting the native llama.cpp server".to_owned()),
            };
            generation
        };

        let args = build_server_args(&model, &profile, port)?;
        let mut command = Command::new(&executable);
        command
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());

        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
        }

        let spawned = command.spawn().map_err(|error| {
            InferenceError::WorkerUnavailable(format!(
                "could not start llama.cpp at {}: {error}",
                executable.display()
            ))
        });

        let mut child = match spawned {
            Ok(child) => Some(child),
            Err(error) => {
                self.mark_start_error(generation, error.to_string())?;
                return Err(error);
            }
        };

        let accepted = {
            let mut state = self.lock_state()?;
            if state.generation == generation && state.child.is_none() {
                state.status.phase = NativeRuntimePhase::Loading;
                state.status.process_id = child.as_ref().map(Child::id);
                state.status.message =
                    Some("llama.cpp is loading GGUF tensors; waiting for health".to_owned());
                state.child = child.take();
                true
            } else {
                false
            }
        };

        if !accepted {
            if let Some(mut child) = child {
                let _ = child.kill();
                let _ = child.wait();
            }
            return Err(InferenceError::Cancelled);
        }

        let deadline = Instant::now() + STARTUP_TIMEOUT;
        loop {
            self.ensure_running(generation)?;
            if Instant::now() >= deadline {
                return self.fail_start(
                    generation,
                    "llama.cpp did not become healthy before the startup timeout".to_owned(),
                );
            }

            match self.http.get(format!("{endpoint}/health")).send().await {
                Ok(response) if response.status().is_success() => break,
                Ok(response) if response.status() == StatusCode::NOT_FOUND => {
                    return self.fail_start(
                        generation,
                        "the llama.cpp server does not expose /health".to_owned(),
                    );
                }
                Ok(response) if response.status().is_client_error() => {
                    return self.fail_start(
                        generation,
                        format!(
                            "llama.cpp health endpoint rejected the request with HTTP {}",
                            response.status()
                        ),
                    );
                }
                Ok(_) | Err(_) => {
                    sleep(HEALTH_POLL_INTERVAL).await;
                }
            }
        }

        let mut state = self.lock_state()?;
        if state.generation != generation {
            return Err(InferenceError::Cancelled);
        }
        Self::refresh_locked(&mut state)?;
        if state.child.is_none() {
            return Err(InferenceError::WorkerUnavailable(
                "llama.cpp exited immediately after reporting health".to_owned(),
            ));
        }
        state.status.phase = NativeRuntimePhase::Ready;
        state.status.message = Some("native llama.cpp runtime loaded the GGUF model".to_owned());
        Ok(state.status.clone())
    }

    pub fn stop(&self) -> Result<NativeRuntimeStatus, InferenceError> {
        let child = {
            let mut state = self.lock_state()?;
            Self::refresh_locked(&mut state)?;
            state.generation = state.generation.wrapping_add(1);
            let child = state.child.take();
            if child.is_some() {
                state.status.phase = NativeRuntimePhase::Stopping;
                state.status.message = Some("stopping the native llama.cpp server".to_owned());
            }
            child
        };

        if let Some(mut child) = child {
            let _ = child.kill();
            let _ = child.wait();
        }

        let mut state = self.lock_state()?;
        state.status = NativeRuntimeStatus::stopped();
        Ok(state.status.clone())
    }

    fn lock_state(&self) -> Result<MutexGuard<'_, RuntimeState>, InferenceError> {
        self.state
            .lock()
            .map_err(|_| InferenceError::WorkerUnavailable("native runtime state lock poisoned".to_owned()))
    }

    fn ensure_running(&self, generation: u64) -> Result<(), InferenceError> {
        let mut state = self.lock_state()?;
        if state.generation != generation {
            return Err(InferenceError::Cancelled);
        }
        Self::refresh_locked(&mut state)?;
        if state.child.is_none() {
            return Err(InferenceError::WorkerUnavailable(
                state
                    .status
                    .message
                    .clone()
                    .unwrap_or_else(|| "llama.cpp exited before becoming healthy".to_owned()),
            ));
        }
        Ok(())
    }

    fn mark_start_error(
        &self,
        generation: u64,
        message: String,
    ) -> Result<(), InferenceError> {
        let mut state = self.lock_state()?;
        if state.generation == generation {
            state.status.phase = NativeRuntimePhase::Error;
            state.status.process_id = None;
            state.status.message = Some(message);
        }
        Ok(())
    }

    fn fail_start(
        &self,
        generation: u64,
        message: String,
    ) -> Result<NativeRuntimeStatus, InferenceError> {
        let child = {
            let mut state = self.lock_state()?;
            if state.generation != generation {
                return Err(InferenceError::Cancelled);
            }
            let child = state.child.take();
            state.status.phase = NativeRuntimePhase::Error;
            state.status.process_id = None;
            state.status.message = Some(message.clone());
            child
        };

        if let Some(mut child) = child {
            let _ = child.kill();
            let _ = child.wait();
        }

        Err(InferenceError::WorkerUnavailable(message))
    }

    fn refresh_locked(state: &mut RuntimeState) -> Result<(), InferenceError> {
        let exit = match state.child.as_mut() {
            Some(child) => child
                .try_wait()
                .map_err(|error| {
                    InferenceError::WorkerUnavailable(format!(
                        "could not inspect the llama.cpp process: {error}"
                    ))
                })?,
            None => return Ok(()),
        };

        let Some(exit) = exit else {
            return Ok(());
        };

        state.child.take();
        state.status.process_id = None;
        state.status.endpoint = None;
        if exit.success() {
            state.status.phase = NativeRuntimePhase::Stopped;
            state.status.message = Some("llama.cpp server exited".to_owned());
        } else {
            state.status.phase = NativeRuntimePhase::Error;
            state.status.message = Some(format!(
                "llama.cpp server exited with status {}",
                exit.code()
                    .map(|code| code.to_string())
                    .unwrap_or_else(|| "unknown".to_owned())
            ));
        }
        Ok(())
    }
}

impl Drop for LlamaServerRuntime {
    fn drop(&mut self) {
        let Ok(state) = self.state.get_mut() else {
            return;
        };
        if let Some(mut child) = state.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn validate_native_model(model: &ModelDescriptor) -> Result<(), InferenceError> {
    if model.format != ModelFormat::Gguf {
        return Err(InferenceError::IncompatibleModel(
            "the native llama.cpp runtime accepts GGUF models only".to_owned(),
        ));
    }
    if !model.path.is_absolute() {
        return Err(InferenceError::IncompatibleModel(
            "the GGUF model path must be absolute".to_owned(),
        ));
    }
    let metadata = std::fs::metadata(&model.path).map_err(|error| {
        InferenceError::IncompatibleModel(format!(
            "the GGUF model file cannot be opened: {error}"
        ))
    })?;
    if !metadata.is_file() {
        return Err(InferenceError::IncompatibleModel(
            "the GGUF model path is not a regular file".to_owned(),
        ));
    }
    if metadata.len() != model.file_size_bytes {
        return Err(InferenceError::IncompatibleModel(
            "the GGUF model changed after its catalog scan; scan it again".to_owned(),
        ));
    }
    Ok(())
}

fn validate_executable(path: &Path) -> Result<PathBuf, InferenceError> {
    let value = path.to_string_lossy();
    if value.trim().is_empty() {
        return Err(InferenceError::WorkerUnavailable(
            "llama.cpp executable path is empty".to_owned(),
        ));
    }

    if is_bare_command(path) {
        return Ok(path.to_path_buf());
    }

    if !path.is_file() {
        return Err(InferenceError::WorkerUnavailable(format!(
            "llama.cpp executable was not found: {}",
            path.display()
        )));
    }

    std::fs::canonicalize(path).map_err(|error| {
        InferenceError::WorkerUnavailable(format!(
            "llama.cpp executable could not be resolved: {error}"
        ))
    })
}

fn is_bare_command(path: &Path) -> bool {
    let value = path.to_string_lossy();
    path.components().count() == 1 && !value.contains('/') && !value.contains('\\')
}

fn build_server_args(
    model: &ModelDescriptor,
    profile: &LoadProfile,
    port: u16,
) -> Result<Vec<OsString>, InferenceError> {
    let mut args = Vec::new();
    push_path_arg(&mut args, "--model", &model.path);
    push_arg(&mut args, "--alias", &model.id);
    push_arg(&mut args, "--host", "127.0.0.1");
    push_arg(&mut args, "--port", port.to_string());
    push_arg(&mut args, "--ctx-size", profile.context_length.to_string());
    push_arg(&mut args, "--threads", profile.cpu_threads.to_string());
    push_arg(&mut args, "--threads-batch", profile.cpu_threads.to_string());
    push_arg(&mut args, "--batch-size", profile.batch_size.to_string());
    push_arg(
        &mut args,
        "--ubatch-size",
        profile.physical_batch_size.to_string(),
    );
    push_arg(
        &mut args,
        "--parallel",
        profile.parallel_requests.to_string(),
    );
    push_arg(
        &mut args,
        "--flash-attn",
        if profile.flash_attention { "on" } else { "off" },
    );
    push_arg(
        &mut args,
        "--cache-type-k",
        cache_type_name(profile.k_cache_quantization)?.to_owned(),
    );
    push_arg(
        &mut args,
        "--cache-type-v",
        cache_type_name(profile.v_cache_quantization)?.to_owned(),
    );
    push_arg(
        &mut args,
        "--n-gpu-layers",
        gpu_layer_count(model.layer_count, profile.gpu_offload_percent).to_string(),
    );

    if profile.kv_cache_offload {
        args.push(OsString::from("--kv-offload"));
    } else {
        args.push(OsString::from("--no-kv-offload"));
    }
    if profile.mmap {
        args.push(OsString::from("--mmap"));
    } else {
        args.push(OsString::from("--no-mmap"));
    }
    if profile.mlock {
        args.push(OsString::from("--mlock"));
    }

    push_arg(
        &mut args,
        "--reasoning",
        if profile.reasoning_enabled { "on" } else { "off" },
    );
    if let Some(budget) = profile.reasoning_budget_tokens {
        push_arg(&mut args, "--reasoning-budget", budget.to_string());
    }
    push_arg(&mut args, "--temp", profile.temperature.to_string());
    push_arg(&mut args, "--top-p", profile.top_p.to_string());
    push_arg(&mut args, "--top-k", profile.top_k.to_string());
    push_arg(&mut args, "--min-p", profile.min_p.to_string());
    push_arg(
        &mut args,
        "--repeat-penalty",
        profile.repeat_penalty.to_string(),
    );
    if let Some(seed) = profile.seed {
        push_arg(&mut args, "--seed", seed.to_string());
    }

    args.push(OsString::from("--no-ui"));
    args.push(OsString::from("--metrics"));
    args.push(OsString::from("--offline"));
    args.push(OsString::from("--jinja"));
    Ok(args)
}

fn push_arg(args: &mut Vec<OsString>, name: &str, value: impl Into<String>) {
    args.push(OsString::from(name));
    args.push(OsString::from(value.into()));
}

fn push_path_arg(args: &mut Vec<OsString>, name: &str, value: &Path) {
    args.push(OsString::from(name));
    args.push(value.as_os_str().to_owned());
}

fn cache_type_name(value: CacheQuantization) -> Result<&'static str, InferenceError> {
    match value {
        CacheQuantization::F32 => Ok("f32"),
        CacheQuantization::F16 => Ok("f16"),
        CacheQuantization::Q8 => Ok("q8_0"),
        CacheQuantization::Q4 => Ok("q4_0"),
        CacheQuantization::Q6 => Err(InferenceError::IncompatibleModel(
            "llama.cpp server does not expose q6 KV-cache quantization in this build".to_owned(),
        )),
    }
}

fn gpu_layer_count(layer_count: Option<u32>, offload_percent: u8) -> i32 {
    match (layer_count, offload_percent) {
        (_, 0) => 0,
        (Some(layers), percent) => {
            ((layers as u64 * percent as u64).saturating_add(99) / 100) as i32
        }
        (None, 100) => -1,
        (None, _) => 0,
    }
}

fn unix_now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gpu_offload_uses_model_layer_count() {
        assert_eq!(gpu_layer_count(Some(32), 0), 0);
        assert_eq!(gpu_layer_count(Some(32), 75), 24);
        assert_eq!(gpu_layer_count(Some(32), 100), 32);
        assert_eq!(gpu_layer_count(None, 100), -1);
    }

    #[test]
    fn q6_cache_is_rejected_until_the_cli_supports_it() {
        assert!(cache_type_name(CacheQuantization::Q6).is_err());
        assert_eq!(cache_type_name(CacheQuantization::Q8).expect("q8"), "q8_0");
    }

    #[test]
    fn server_args_pin_model_alias_and_disable_web_ui() {
        let model = ModelDescriptor {
            id: "catalog-model-id".to_owned(),
            display_name: "Catalog model".to_owned(),
            path: PathBuf::from("model.gguf"),
            format: ModelFormat::Gguf,
            family: None,
            parameter_count: None,
            architecture: None,
            quantization: None,
            gguf_version: None,
            file_size_bytes: 1,
            context_capacity: None,
            layer_count: None,
            attention_head_count: None,
            key_value_head_count: None,
            embedding_length: None,
            bits_per_weight: None,
            capabilities: Default::default(),
        };
        let args = build_server_args(&model, &LoadProfile::eco(), 45_678)
            .expect("server args");
        let args = args
            .iter()
            .map(|value| value.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        let alias_index = args
            .iter()
            .position(|value| value == "--alias")
            .expect("alias");
        assert_eq!(args[alias_index + 1], model.id);
        assert!(args.iter().any(|value| value == "--no-ui"));
    }

    #[test]
    fn native_status_starts_stopped() {
        assert_eq!(
            NativeRuntimeStatus::stopped().phase,
            NativeRuntimePhase::Stopped
        );
    }
}
