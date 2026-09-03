//! Supervision and streaming client for an isolated llama.cpp server process.

use aegis_inference::{LoadPreset, LoadProfile};
use futures_util::StreamExt;
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};
use thiserror::Error;
use tokio::process::{Child, Command};
use tokio::sync::{mpsc, Mutex, RwLock};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

const LOOPBACK: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatRequest {
    pub messages: Vec<ChatMessage>,
    pub max_tokens: u32,
    pub temperature: f32,
    pub top_p: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatDelta {
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompletionSummary {
    pub prompt_tokens: Option<u64>,
    pub generated_tokens: Option<u64>,
    pub elapsed_ms: u64,
    pub tokens_per_second: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RuntimeSnapshot {
    pub running: bool,
    pub model_path: Option<PathBuf>,
    pub model_name: Option<String>,
    pub profile: Option<LoadPreset>,
    pub context_length: Option<u32>,
    pub accelerator: Option<String>,
    pub port: Option<u16>,
    pub tokens_per_second: Option<f64>,
    pub last_error: Option<String>,
}

impl Default for RuntimeSnapshot {
    fn default() -> Self {
        Self {
            running: false,
            model_path: None,
            model_name: None,
            profile: None,
            context_length: None,
            accelerator: None,
            port: None,
            tokens_per_second: None,
            last_error: None,
        }
    }
}

struct WorkerState {
    child: Option<Child>,
    endpoint: Option<String>,
    api_key: Option<String>,
}

pub struct LlamaServerRuntime {
    client: Client,
    worker: Mutex<WorkerState>,
    snapshot: RwLock<RuntimeSnapshot>,
}

impl Default for LlamaServerRuntime {
    fn default() -> Self {
        Self::new()
    }
}

impl LlamaServerRuntime {
    pub fn new() -> Self {
        Self {
            client: Client::builder()
                .connect_timeout(Duration::from_secs(2))
                .timeout(Duration::from_secs(180))
                .build()
                .expect("static HTTP client configuration must be valid"),
            worker: Mutex::new(WorkerState {
                child: None,
                endpoint: None,
                api_key: None,
            }),
            snapshot: RwLock::new(RuntimeSnapshot::default()),
        }
    }

    pub async fn snapshot(&self) -> RuntimeSnapshot {
        self.snapshot.read().await.clone()
    }

    pub async fn start(
        &self,
        executable: &Path,
        model: &Path,
        profile: LoadProfile,
        cancellation: CancellationToken,
    ) -> Result<RuntimeSnapshot, RuntimeError> {
        profile.validate().map_err(|error| RuntimeError::InvalidConfiguration(error.to_string()))?;
        let executable = canonical_file(executable, "llama-server executable")?;
        let model = canonical_file(model, "GGUF model")?;
        if !model
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("gguf"))
        {
            return Err(RuntimeError::InvalidConfiguration(
                "selected model must use the .gguf extension".to_owned(),
            ));
        }

        self.stop().await?;
        let port = reserve_loopback_port()?;
        let api_key = format!("aegis-{}", Uuid::new_v4());
        let gpu_layers = match profile.gpu_offload_percent {
            0 => 0,
            100 => 999,
            percent => u32::from(percent),
        };

        let mut command = Command::new(&executable);
        command
            .arg("--model")
            .arg(&model)
            .arg("--host")
            .arg("127.0.0.1")
            .arg("--port")
            .arg(port.to_string())
            .arg("--ctx-size")
            .arg(profile.context_length.to_string())
            .arg("--threads")
            .arg(profile.cpu_threads.to_string())
            .arg("--batch-size")
            .arg(profile.batch_size.to_string())
            .arg("--ubatch-size")
            .arg(profile.physical_batch_size.to_string())
            .arg("--n-gpu-layers")
            .arg(gpu_layers.to_string())
            .arg("--parallel")
            .arg(profile.parallel_requests.to_string())
            .arg("--api-key")
            .arg(&api_key)
            .arg("--no-webui")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .creation_flags(no_window_flag());

        let child = command.spawn().map_err(RuntimeError::Spawn)?;
        let endpoint = format!("http://127.0.0.1:{port}");
        {
            let mut worker = self.worker.lock().await;
            worker.child = Some(child);
            worker.endpoint = Some(endpoint.clone());
            worker.api_key = Some(api_key.clone());
        }

        let readiness = self
            .wait_until_ready(&endpoint, &api_key, cancellation)
            .await;
        if let Err(error) = readiness {
            let _ = self.stop().await;
            self.snapshot.write().await.last_error = Some(error.to_string());
            return Err(error);
        }

        let model_name = model
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("GGUF model")
            .to_owned();
        let snapshot = RuntimeSnapshot {
            running: true,
            model_path: Some(model),
            model_name: Some(model_name),
            profile: Some(profile.preset),
            context_length: Some(profile.context_length),
            accelerator: Some(if profile.gpu_offload_percent == 0 {
                "CPU".to_owned()
            } else {
                "Vulkan".to_owned()
            }),
            port: Some(port),
            tokens_per_second: None,
            last_error: None,
        };
        *self.snapshot.write().await = snapshot.clone();
        Ok(snapshot)
    }

    pub async fn stop(&self) -> Result<(), RuntimeError> {
        let mut worker = self.worker.lock().await;
        if let Some(mut child) = worker.child.take() {
            child.kill().await.map_err(RuntimeError::Stop)?;
            let _ = child.wait().await;
        }
        worker.endpoint = None;
        worker.api_key = None;
        self.snapshot.write().await.running = false;
        Ok(())
    }

    pub async fn stream_chat(
        &self,
        request: ChatRequest,
        deltas: mpsc::Sender<ChatDelta>,
        cancellation: CancellationToken,
    ) -> Result<CompletionSummary, RuntimeError> {
        if request.messages.is_empty() {
            return Err(RuntimeError::InvalidConfiguration(
                "at least one chat message is required".to_owned(),
            ));
        }
        let (endpoint, api_key) = {
            let worker = self.worker.lock().await;
            (
                worker.endpoint.clone().ok_or(RuntimeError::NotRunning)?,
                worker.api_key.clone().ok_or(RuntimeError::NotRunning)?,
            )
        };
        let started = Instant::now();
        let response = self
            .client
            .post(format!("{endpoint}/v1/chat/completions"))
            .bearer_auth(api_key)
            .json(&json!({
                "messages": request.messages,
                "stream": true,
                "stream_options": {"include_usage": true},
                "max_tokens": request.max_tokens,
                "temperature": request.temperature,
                "top_p": request.top_p
            }))
            .send()
            .await
            .map_err(RuntimeError::Http)?;
        if !response.status().is_success() {
            return Err(http_status_error(response).await);
        }

        let mut stream = response.bytes_stream();
        let mut pending = String::new();
        let mut generated_tokens = None;
        let mut prompt_tokens = None;
        loop {
            let next = tokio::select! {
                _ = cancellation.cancelled() => return Err(RuntimeError::Cancelled),
                next = stream.next() => next,
            };
            let Some(chunk) = next else { break };
            let chunk = chunk.map_err(RuntimeError::Http)?;
            pending.push_str(&String::from_utf8_lossy(&chunk));
            while let Some(newline) = pending.find('\n') {
                let line = pending[..newline].trim().to_owned();
                pending.drain(..=newline);
                let Some(data) = line.strip_prefix("data:") else { continue };
                let data = data.trim();
                if data == "[DONE]" {
                    continue;
                }
                let value: serde_json::Value = serde_json::from_str(data)
                    .map_err(|error| RuntimeError::Protocol(error.to_string()))?;
                if let Some(text) = value
                    .pointer("/choices/0/delta/content")
                    .and_then(serde_json::Value::as_str)
                {
                    if !text.is_empty() && deltas.send(ChatDelta { text: text.to_owned() }).await.is_err() {
                        return Err(RuntimeError::Cancelled);
                    }
                }
                if let Some(usage) = value.get("usage") {
                    prompt_tokens = usage.get("prompt_tokens").and_then(serde_json::Value::as_u64);
                    generated_tokens = usage
                        .get("completion_tokens")
                        .and_then(serde_json::Value::as_u64);
                }
            }
        }
        let elapsed_ms = started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
        let tokens_per_second = generated_tokens.filter(|_| elapsed_ms > 0).map(|tokens| {
            tokens as f64 / (elapsed_ms as f64 / 1000.0)
        });
        self.snapshot.write().await.tokens_per_second = tokens_per_second;
        Ok(CompletionSummary {
            prompt_tokens,
            generated_tokens,
            elapsed_ms,
            tokens_per_second,
        })
    }

    async fn wait_until_ready(
        &self,
        endpoint: &str,
        api_key: &str,
        cancellation: CancellationToken,
    ) -> Result<(), RuntimeError> {
        let deadline = Instant::now() + Duration::from_secs(180);
        loop {
            if cancellation.is_cancelled() {
                return Err(RuntimeError::Cancelled);
            }
            {
                let mut worker = self.worker.lock().await;
                if let Some(child) = worker.child.as_mut() {
                    if let Some(status) = child.try_wait().map_err(RuntimeError::Spawn)? {
                        return Err(RuntimeError::Exited(status.code()));
                    }
                }
            }
            match self
                .client
                .get(format!("{endpoint}/health"))
                .bearer_auth(api_key)
                .send()
                .await
            {
                Ok(response) if response.status() == StatusCode::OK => return Ok(()),
                Ok(response) if response.status() == StatusCode::SERVICE_UNAVAILABLE => {}
                Ok(_) | Err(_) => {}
            }
            if Instant::now() >= deadline {
                return Err(RuntimeError::ReadinessTimeout);
            }
            tokio::time::sleep(Duration::from_millis(400)).await;
        }
    }
}

fn canonical_file(path: &Path, label: &'static str) -> Result<PathBuf, RuntimeError> {
    let canonical = path
        .canonicalize()
        .map_err(|source| RuntimeError::InvalidPath { label, source })?;
    if !canonical.is_file() {
        return Err(RuntimeError::InvalidConfiguration(format!(
            "{label} is not a file"
        )));
    }
    Ok(canonical)
}

fn reserve_loopback_port() -> Result<u16, RuntimeError> {
    let listener = TcpListener::bind(SocketAddr::new(LOOPBACK, 0)).map_err(RuntimeError::Port)?;
    listener.local_addr().map(|address| address.port()).map_err(RuntimeError::Port)
}

async fn http_status_error(response: reqwest::Response) -> RuntimeError {
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    RuntimeError::Protocol(format!("llama.cpp returned {status}: {}", body.chars().take(500).collect::<String>()))
}

#[cfg(windows)]
fn no_window_flag() -> u32 {
    0x08000000
}

#[cfg(not(windows))]
fn no_window_flag() -> u32 {
    0
}

trait CommandCreationFlags {
    fn creation_flags(&mut self, flags: u32) -> &mut Self;
}

#[cfg(windows)]
impl CommandCreationFlags for Command {
    fn creation_flags(&mut self, flags: u32) -> &mut Self {
        use std::os::windows::process::CommandExt;
        self.as_std_mut().creation_flags(flags);
        self
    }
}

#[cfg(not(windows))]
impl CommandCreationFlags for Command {
    fn creation_flags(&mut self, _flags: u32) -> &mut Self {
        self
    }
}

#[derive(Debug, Error)]
pub enum RuntimeError {
    #[error("invalid runtime configuration: {0}")]
    InvalidConfiguration(String),
    #[error("{label} path is invalid: {source}")]
    InvalidPath { label: &'static str, source: std::io::Error },
    #[error("could not reserve a loopback port: {0}")]
    Port(std::io::Error),
    #[error("could not start llama.cpp worker: {0}")]
    Spawn(std::io::Error),
    #[error("could not stop llama.cpp worker: {0}")]
    Stop(std::io::Error),
    #[error("llama.cpp worker exited before becoming ready (code {0:?})")]
    Exited(Option<i32>),
    #[error("llama.cpp worker did not become ready within 180 seconds")]
    ReadinessTimeout,
    #[error("llama.cpp worker is not running")]
    NotRunning,
    #[error("inference request failed: {0}")]
    Http(reqwest::Error),
    #[error("inference protocol error: {0}")]
    Protocol(String),
    #[error("inference operation was cancelled")]
    Cancelled,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_existing_gguf_files_are_accepted() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let wrong = directory.path().join("model.bin");
        std::fs::write(&wrong, b"test").expect("test file");
        assert!(canonical_file(&wrong, "model").is_ok());
        assert!(!wrong.extension().is_some_and(|extension| extension.eq_ignore_ascii_case("gguf")));
    }

    #[test]
    fn loopback_port_is_dynamic() {
        assert_ne!(reserve_loopback_port().expect("port"), 0);
    }
}
