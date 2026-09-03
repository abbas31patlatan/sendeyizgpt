//! Tool manifests, broker-mediated invocation and untrusted output types.

use aegis_permissions::{
    ActionRequest, ApprovalRequest, Capability, ExecutionPermit, PermissionBroker,
    PermissionDecision, PermissionError, RiskLevel,
};
use aegis_protocol::RequestId;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;
use thiserror::Error;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolOrigin {
    BuiltIn,
    SignedPlugin,
    McpServer,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolLimits {
    pub max_input_bytes: usize,
    pub max_output_bytes: usize,
    pub max_duration_ms: u64,
}

impl Default for ToolLimits {
    fn default() -> Self {
        Self {
            max_input_bytes: 1_048_576,
            max_output_bytes: 4_194_304,
            max_duration_ms: 120_000,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolManifest {
    pub id: String,
    pub name: String,
    pub description: String,
    pub version: String,
    pub capabilities: Vec<Capability>,
    pub input_schema: Value,
    pub output_schema: Value,
    pub risk_level: RiskLevel,
    pub origin: ToolOrigin,
    pub limits: ToolLimits,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    pub call_id: RequestId,
    pub tool_id: String,
    pub input: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UntrustedContent {
    pub source: String,
    pub text: String,
    pub content_hash: String,
    pub truncated: bool,
}

impl UntrustedContent {
    pub fn from_text(source: impl Into<String>, text: String, max_bytes: usize) -> Self {
        let content_hash = hex_encode(blake3::hash(text.as_bytes()).as_bytes());
        let truncated = text.len() > max_bytes;
        let bounded_text = if truncated {
            String::from_utf8_lossy(&text.as_bytes()[..max_bytes]).into_owned()
        } else {
            text
        };
        Self {
            source: source.into(),
            text: bounded_text,
            content_hash,
            truncated,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolResult {
    pub call_id: RequestId,
    pub content: UntrustedContent,
    pub duration_ms: u64,
    pub exit_code: Option<i32>,
}

#[async_trait]
pub trait ToolExecutor: Send + Sync {
    fn manifest(&self) -> &ToolManifest;

    /// Builds the preview/action request. This method must not cause an effect.
    async fn preview(&self, call: &ToolCall) -> Result<ActionRequest, ToolError>;

    /// Executes only after the runtime has consumed a broker permit.
    async fn execute(
        &self,
        call: ToolCall,
        permit: ExecutionPermit,
        cancellation: CancellationToken,
    ) -> Result<ToolResult, ToolError>;
}

#[derive(Debug)]
struct PendingInvocation {
    call: ToolCall,
    action: ActionRequest,
    tool_id: String,
}

#[derive(Debug)]
pub enum ToolInvocation {
    Completed(ToolResult),
    ApprovalRequired { approval: Box<ApprovalRequest> },
    Denied { reason: String },
}

pub struct ToolRuntime {
    broker: Arc<Mutex<PermissionBroker>>,
    executors: RwLock<HashMap<String, Arc<dyn ToolExecutor>>>,
    pending: Mutex<HashMap<Uuid, PendingInvocation>>,
}

impl ToolRuntime {
    pub fn new(broker: Arc<Mutex<PermissionBroker>>) -> Self {
        Self {
            broker,
            executors: RwLock::new(HashMap::new()),
            pending: Mutex::new(HashMap::new()),
        }
    }

    pub fn register(&self, executor: Arc<dyn ToolExecutor>) -> Result<(), ToolError> {
        let manifest = executor.manifest();
        if manifest.id.trim().is_empty() {
            return Err(ToolError::InvalidManifest("tool id is required".to_owned()));
        }
        if manifest.limits.max_input_bytes == 0
            || manifest.limits.max_output_bytes == 0
            || manifest.limits.max_duration_ms == 0
        {
            return Err(ToolError::InvalidManifest(
                "tool limits must all be positive".to_owned(),
            ));
        }
        let mut executors = self
            .executors
            .write()
            .map_err(|_| ToolError::LockPoisoned("tool registry"))?;
        if executors.contains_key(&manifest.id) {
            return Err(ToolError::DuplicateTool(manifest.id.clone()));
        }
        executors.insert(manifest.id.clone(), executor);
        Ok(())
    }

    pub async fn propose(
        &self,
        call: ToolCall,
        cancellation: CancellationToken,
    ) -> Result<ToolInvocation, ToolError> {
        let executor = self.executor_for(&call.tool_id)?;
        let manifest = executor.manifest().clone();
        let input_bytes = serde_json::to_vec(&call.input)
            .map_err(|error| ToolError::InvalidInput(error.to_string()))?;
        if input_bytes.len() > manifest.limits.max_input_bytes {
            return Err(ToolError::InputTooLarge {
                actual: input_bytes.len(),
                maximum: manifest.limits.max_input_bytes,
            });
        }
        validate_input(&manifest, &call.input)?;
        let action = executor.preview(&call).await?;
        if action.tool_id != manifest.id {
            return Err(ToolError::ManifestMismatch {
                expected: manifest.id.clone(),
                actual: action.tool_id.clone(),
            });
        }
        if action.risk > manifest.risk_level
            || !action
                .capabilities
                .iter()
                .all(|capability| manifest.capabilities.contains(capability))
        {
            return Err(ToolError::ManifestMismatch {
                expected: manifest.id.clone(),
                actual: "preview exceeds declared capability/risk".to_owned(),
            });
        }
        let decision = self
            .broker
            .lock()
            .map_err(|_| ToolError::LockPoisoned("permission broker"))?
            .evaluate(action.clone())?;

        match decision {
            PermissionDecision::AutoApproved { permit } => {
                self.broker
                    .lock()
                    .map_err(|_| ToolError::LockPoisoned("permission broker"))?
                    .consume_permit(permit.clone(), &action)?;
                let result = execute_with_limits(
                    executor.clone(),
                    call,
                    permit,
                    cancellation,
                    &manifest.limits,
                )
                .await?;
                validate_output(&manifest, &result)?;
                Ok(ToolInvocation::Completed(result))
            }
            PermissionDecision::ApprovalRequired { approval } => {
                let approval_id = approval.approval_id;
                self.pending
                    .lock()
                    .map_err(|_| ToolError::LockPoisoned("pending tool calls"))?
                    .insert(
                        approval_id,
                        PendingInvocation {
                            tool_id: call.tool_id.clone(),
                            call,
                            action,
                        },
                    );
                Ok(ToolInvocation::ApprovalRequired { approval })
            }
            PermissionDecision::Denied { reason } => Ok(ToolInvocation::Denied { reason }),
        }
    }

    pub async fn approve_and_execute(
        &self,
        approval_id: Uuid,
        confirmation_nonce: &str,
        cancellation: CancellationToken,
    ) -> Result<ToolInvocation, ToolError> {
        let pending = self
            .pending
            .lock()
            .map_err(|_| ToolError::LockPoisoned("pending tool calls"))?
            .remove(&approval_id)
            .ok_or(ToolError::PendingInvocationNotFound(approval_id))?;
        // Resolve and validate the executor before turning the approval into
        // a permit. A registry failure must not strand an approved permit.
        let executor = match self.executor_for(&pending.tool_id) {
            Ok(executor) => executor,
            Err(error) => {
                self.pending
                    .lock()
                    .map_err(|_| ToolError::LockPoisoned("pending tool calls"))?
                    .insert(approval_id, pending);
                return Err(error);
            }
        };
        let manifest = executor.manifest().clone();
        if let Err(error) = validate_input(&manifest, &pending.call.input) {
            self.pending
                .lock()
                .map_err(|_| ToolError::LockPoisoned("pending tool calls"))?
                .insert(approval_id, pending);
            return Err(error);
        }
        let permit_result = self
            .broker
            .lock()
            .map_err(|_| ToolError::LockPoisoned("permission broker"))?
            .approve(approval_id, confirmation_nonce);
        let permit = match permit_result {
            Ok(permit) => permit,
            Err(error) => {
                self.pending
                    .lock()
                    .map_err(|_| ToolError::LockPoisoned("pending tool calls"))?
                    .insert(approval_id, pending);
                return Err(error.into());
            }
        };
        self.broker
            .lock()
            .map_err(|_| ToolError::LockPoisoned("permission broker"))?
            .consume_permit(permit.clone(), &pending.action)?;
        let result = execute_with_limits(
            executor.clone(),
            pending.call,
            permit,
            cancellation,
            &manifest.limits,
        )
        .await?;
        validate_output(&manifest, &result)?;
        Ok(ToolInvocation::Completed(result))
    }

    fn executor_for(&self, tool_id: &str) -> Result<Arc<dyn ToolExecutor>, ToolError> {
        self.executors
            .read()
            .map_err(|_| ToolError::LockPoisoned("tool registry"))?
            .get(tool_id)
            .cloned()
            .ok_or_else(|| ToolError::UnknownTool(tool_id.to_owned()))
    }
}

fn validate_input(manifest: &ToolManifest, input: &Value) -> Result<(), ToolError> {
    let validator = jsonschema::validator_for(&manifest.input_schema)
        .map_err(|error| ToolError::InvalidSchema(error.to_string()))?;
    validator
        .validate(input)
        .map_err(|error| ToolError::InvalidInput(error.to_string()))
}

fn validate_output(manifest: &ToolManifest, result: &ToolResult) -> Result<(), ToolError> {
    let output_bytes =
        serde_json::to_vec(result).map_err(|error| ToolError::InvalidInput(error.to_string()))?;
    if output_bytes.len() > manifest.limits.max_output_bytes {
        return Err(ToolError::OutputTooLarge {
            actual: output_bytes.len(),
            maximum: manifest.limits.max_output_bytes,
        });
    }
    let output = serde_json::from_slice::<Value>(&output_bytes)
        .map_err(|error| ToolError::InvalidOutput(error.to_string()))?;
    let validator = jsonschema::validator_for(&manifest.output_schema)
        .map_err(|error| ToolError::InvalidSchema(error.to_string()))?;
    validator
        .validate(&output)
        .map_err(|error| ToolError::InvalidOutput(error.to_string()))
}

async fn execute_with_limits(
    executor: Arc<dyn ToolExecutor>,
    call: ToolCall,
    permit: ExecutionPermit,
    cancellation: CancellationToken,
    limits: &ToolLimits,
) -> Result<ToolResult, ToolError> {
    let executor_cancellation = cancellation.child_token();
    let cancellation_for_executor = executor_cancellation.clone();
    let execution = async move {
        executor
            .execute(call, permit, cancellation_for_executor.clone())
            .await
    };
    tokio::select! {
        result = tokio::time::timeout(Duration::from_millis(limits.max_duration_ms), execution) => {
            match result {
                Ok(result) => result,
                Err(_) => {
                    executor_cancellation.cancel();
                    Err(ToolError::TimedOut { duration_ms: limits.max_duration_ms })
                },
            }
        }
        _ = cancellation.cancelled() => {
            executor_cancellation.cancel();
            Err(ToolError::Cancelled)
        },
    }
}

#[derive(Debug, Error)]
pub enum ToolError {
    #[error("permission error: {0}")]
    Permission(#[from] PermissionError),
    #[error("unknown tool: {0}")]
    UnknownTool(String),
    #[error("duplicate tool: {0}")]
    DuplicateTool(String),
    #[error("invalid tool manifest: {0}")]
    InvalidManifest(String),
    #[error("tool input is not valid JSON: {0}")]
    InvalidInput(String),
    #[error("tool input is too large: {actual} bytes, maximum {maximum} bytes")]
    InputTooLarge { actual: usize, maximum: usize },
    #[error("tool output is too large: {actual} bytes, maximum {maximum} bytes")]
    OutputTooLarge { actual: usize, maximum: usize },
    #[error("tool input schema is invalid: {0}")]
    InvalidSchema(String),
    #[error("tool output does not match its declared schema: {0}")]
    InvalidOutput(String),
    #[error("tool exceeded its execution time limit of {duration_ms} ms")]
    TimedOut { duration_ms: u64 },
    #[error("tool execution was cancelled")]
    Cancelled,
    #[error("tool manifest mismatch for {expected}: {actual}")]
    ManifestMismatch { expected: String, actual: String },
    #[error("pending invocation not found: {0}")]
    PendingInvocationNotFound(Uuid),
    #[error("lock poisoned: {0}")]
    LockPoisoned(&'static str),
    #[error("tool execution failed: {0}")]
    Execution(String),
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut result = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        result.push(HEX[(byte >> 4) as usize] as char);
        result.push(HEX[(byte & 0x0f) as usize] as char);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use aegis_permissions::{
        ActionKind, ActionPreview, ActionTarget, CommandPreview, PermissionPolicy,
    };
    use chrono::Utc;
    use std::collections::BTreeSet;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct FakeShell {
        manifest: ToolManifest,
        cwd: PathBuf,
        runs: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl ToolExecutor for FakeShell {
        fn manifest(&self) -> &ToolManifest {
            &self.manifest
        }

        async fn preview(&self, _call: &ToolCall) -> Result<ActionRequest, ToolError> {
            Ok(ActionRequest {
                request_id: RequestId::new(),
                conversation_id: None,
                agent_id: None,
                tool_id: self.manifest.id.clone(),
                action: ActionKind::ExecuteCommand,
                capabilities: BTreeSet::from([Capability::ShellExecute]),
                risk: RiskLevel::High,
                target: ActionTarget::Command {
                    program: "echo".to_owned(),
                    args: vec!["safe".to_owned()],
                    cwd: self.cwd.clone(),
                    network_required: false,
                    environment_keys: Vec::new(),
                },
                preview: ActionPreview {
                    title: "Run a test command".to_owned(),
                    summary: "This executor is used only by the permission test".to_owned(),
                    effects: vec!["Starts a process".to_owned()],
                    diff: None,
                    command: Some(CommandPreview {
                        program: "echo".to_owned(),
                        args: vec!["safe".to_owned()],
                        cwd: self.cwd.clone(),
                        network_required: false,
                        environment_keys: Vec::new(),
                    }),
                },
                requested_at: Utc::now(),
            })
        }

        async fn execute(
            &self,
            call: ToolCall,
            _permit: ExecutionPermit,
            _cancellation: CancellationToken,
        ) -> Result<ToolResult, ToolError> {
            self.runs.fetch_add(1, Ordering::SeqCst);
            Ok(ToolResult {
                call_id: call.call_id,
                content: UntrustedContent::from_text("test-shell", "executed".to_owned(), 1024),
                duration_ms: 1,
                exit_code: Some(0),
            })
        }
    }

    #[test]
    fn external_output_is_marked_and_bounded() {
        let output =
            UntrustedContent::from_text("browser:https://example.test", "abcdef".to_owned(), 3);
        assert_eq!(output.text, "abc");
        assert!(output.truncated);
        assert!(!output.content_hash.is_empty());
    }

    #[test]
    fn input_schema_is_checked_before_preview() {
        let manifest = ToolManifest {
            id: "test".to_owned(),
            name: "Test".to_owned(),
            description: "Test".to_owned(),
            version: "1.0.0".to_owned(),
            capabilities: Vec::new(),
            input_schema: serde_json::json!({
                "type": "object",
                "required": ["path"],
                "properties": {"path": {"type": "string"}}
            }),
            output_schema: serde_json::json!({"type": "object"}),
            risk_level: RiskLevel::ReadOnly,
            origin: ToolOrigin::BuiltIn,
            limits: ToolLimits::default(),
        };
        assert!(validate_input(&manifest, &serde_json::json!({"path": "ok"})).is_ok());
        assert!(validate_input(&manifest, &serde_json::json!({})).is_err());
    }

    #[tokio::test]
    async fn shell_executor_is_not_called_before_explicit_approval() {
        let directory = tempfile::tempdir().expect("temp directory");
        let broker = Arc::new(Mutex::new(
            PermissionBroker::new(PermissionPolicy {
                workspace_roots: vec![directory.path().to_path_buf()],
                ..PermissionPolicy::default()
            })
            .expect("broker creates"),
        ));
        let runtime = ToolRuntime::new(broker.clone());
        let runs = Arc::new(AtomicUsize::new(0));
        runtime
            .register(Arc::new(FakeShell {
                manifest: ToolManifest {
                    id: "test.shell".to_owned(),
                    name: "Test shell".to_owned(),
                    description: "test".to_owned(),
                    version: "1.0.0".to_owned(),
                    capabilities: vec![Capability::ShellExecute],
                    input_schema: serde_json::json!({"type": "object"}),
                    output_schema: serde_json::json!({"type": "object"}),
                    risk_level: RiskLevel::Critical,
                    origin: ToolOrigin::BuiltIn,
                    limits: ToolLimits::default(),
                },
                cwd: directory.path().to_path_buf(),
                runs: runs.clone(),
            }))
            .expect("tool registers");

        let outcome = runtime
            .propose(
                ToolCall {
                    call_id: RequestId::new(),
                    tool_id: "test.shell".to_owned(),
                    input: serde_json::json!({}),
                },
                CancellationToken::new(),
            )
            .await
            .expect("proposal evaluates");
        let approval_id = match outcome {
            ToolInvocation::ApprovalRequired { approval } => approval.approval_id,
            other => panic!("expected approval, got {other:?}"),
        };
        assert_eq!(runs.load(Ordering::SeqCst), 0);

        let nonce = broker
            .lock()
            .expect("broker lock")
            .approval_for_ui(approval_id)
            .expect("approval view")
            .confirmation_nonce;
        runtime
            .approve_and_execute(approval_id, &nonce, CancellationToken::new())
            .await
            .expect("approved execution");
        assert_eq!(runs.load(Ordering::SeqCst), 1);
    }
}
