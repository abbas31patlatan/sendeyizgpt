//! Application orchestration state shared by the Tauri shell and future
//! workers. Effectful operations remain behind the Permission Broker.

use aegis_permissions::PermissionBroker;
use aegis_tools::ToolRuntime;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use thiserror::Error;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoreState {
    Starting,
    Ready,
    Degraded,
    Stopped,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RuntimeStatus {
    pub app_version: String,
    pub core_state: CoreState,
    pub model_name: Option<String>,
    pub backend_name: Option<String>,
    pub accelerator: Option<String>,
    pub gpu_name: Option<String>,
    pub vram_bytes: Option<u64>,
    pub context_length: Option<u32>,
    pub tokens_per_second: Option<f64>,
    pub last_error: Option<String>,
}

impl Default for RuntimeStatus {
    fn default() -> Self {
        Self {
            app_version: env!("CARGO_PKG_VERSION").to_owned(),
            core_state: CoreState::Ready,
            model_name: None,
            backend_name: None,
            accelerator: None,
            gpu_name: None,
            vram_bytes: None,
            context_length: None,
            tokens_per_second: None,
            last_error: None,
        }
    }
}

pub struct ApplicationCore {
    broker: Arc<std::sync::Mutex<PermissionBroker>>,
    tool_runtime: ToolRuntime,
    runtime_status: RwLock<RuntimeStatus>,
    cancellations: std::sync::Mutex<HashMap<Uuid, CancellationToken>>,
}

impl ApplicationCore {
    pub fn new() -> Result<Self, CoreError> {
        let broker = Arc::new(std::sync::Mutex::new(
            PermissionBroker::new(Default::default()).map_err(CoreError::Permission)?,
        ));
        let tool_runtime = ToolRuntime::new(broker.clone());
        Ok(Self {
            broker,
            tool_runtime,
            runtime_status: RwLock::new(RuntimeStatus::default()),
            cancellations: std::sync::Mutex::new(HashMap::new()),
        })
    }

    pub fn broker(&self) -> Arc<std::sync::Mutex<PermissionBroker>> {
        self.broker.clone()
    }

    pub fn tool_runtime(&self) -> &ToolRuntime {
        &self.tool_runtime
    }

    pub fn runtime_status(&self) -> Result<RuntimeStatus, CoreError> {
        self.runtime_status
            .read()
            .map_err(|_| CoreError::LockPoisoned("runtime status"))
            .map(|status| status.clone())
    }

    pub fn set_runtime_status(&self, status: RuntimeStatus) -> Result<(), CoreError> {
        *self
            .runtime_status
            .write()
            .map_err(|_| CoreError::LockPoisoned("runtime status"))? = status;
        Ok(())
    }

    pub fn start_operation(&self) -> Result<(Uuid, CancellationToken), CoreError> {
        let operation_id = Uuid::new_v4();
        let token = CancellationToken::new();
        self.cancellations
            .lock()
            .map_err(|_| CoreError::LockPoisoned("cancellation registry"))?
            .insert(operation_id, token.clone());
        Ok((operation_id, token))
    }

    pub fn finish_operation(&self, operation_id: Uuid) -> Result<(), CoreError> {
        self.cancellations
            .lock()
            .map_err(|_| CoreError::LockPoisoned("cancellation registry"))?
            .remove(&operation_id);
        Ok(())
    }

    pub fn cancel_operation(&self, operation_id: Uuid) -> Result<bool, CoreError> {
        let token = self
            .cancellations
            .lock()
            .map_err(|_| CoreError::LockPoisoned("cancellation registry"))?
            .get(&operation_id)
            .cloned();
        if let Some(token) = token {
            token.cancel();
            Ok(true)
        } else {
            Ok(false)
        }
    }

    pub fn stop_everything(&self) -> Result<usize, CoreError> {
        let tokens: Vec<CancellationToken> = self
            .cancellations
            .lock()
            .map_err(|_| CoreError::LockPoisoned("cancellation registry"))?
            .drain()
            .map(|(_, token)| token)
            .collect();
        let count = tokens.len();
        for token in tokens {
            token.cancel();
        }
        Ok(count)
    }
}

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("permission broker initialization failed: {0}")]
    Permission(aegis_permissions::PermissionError),
    #[error("core state lock poisoned: {0}")]
    LockPoisoned(&'static str),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stop_everything_cancels_registered_operations() {
        let core = ApplicationCore::new().expect("core creates");
        let (_, first) = core.start_operation().expect("operation");
        let (_, second) = core.start_operation().expect("operation");
        assert_eq!(core.stop_everything().expect("stop"), 2);
        assert!(first.is_cancelled());
        assert!(second.is_cancelled());
    }
}
