//! Trusted native permission boundary.
//!
//! This crate intentionally contains no LLM or prompt logic. A model can only
//! cause an `ActionRequest` to be evaluated; the broker decides whether an
//! executor may receive a one-time permit.

use aegis_protocol::RequestId;
use blake3::Hash;
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap};
use std::ffi::OsString;
use std::fs;
use std::path::{Component, Path, PathBuf};
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    FilesystemRead,
    FilesystemWrite,
    FilesystemDelete,
    ShellExecute,
    ProcessStart,
    ProcessStop,
    NetworkHttp,
    BrowserRead,
    BrowserInteract,
    ClipboardRead,
    ClipboardWrite,
    ScreenCapture,
    MicrophoneListen,
    CameraCapture,
    NotificationsSend,
    SystemInfo,
    ApplicationControl,
    KeyboardInject,
    MouseInject,
    GitPush,
    FileUpload,
    PolicyChange,
    SecretRead,
}

impl Capability {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::FilesystemRead => "filesystem.read",
            Self::FilesystemWrite => "filesystem.write",
            Self::FilesystemDelete => "filesystem.delete",
            Self::ShellExecute => "shell.execute",
            Self::ProcessStart => "process.start",
            Self::ProcessStop => "process.stop",
            Self::NetworkHttp => "network.http",
            Self::BrowserRead => "browser.read",
            Self::BrowserInteract => "browser.interact",
            Self::ClipboardRead => "clipboard.read",
            Self::ClipboardWrite => "clipboard.write",
            Self::ScreenCapture => "screen.capture",
            Self::MicrophoneListen => "microphone.listen",
            Self::CameraCapture => "camera.capture",
            Self::NotificationsSend => "notifications.send",
            Self::SystemInfo => "system.info",
            Self::ApplicationControl => "application.control",
            Self::KeyboardInject => "keyboard.inject",
            Self::MouseInject => "mouse.inject",
            Self::GitPush => "git.push",
            Self::FileUpload => "file.upload",
            Self::PolicyChange => "policy.change",
            Self::SecretRead => "secret.read",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RiskLevel {
    ReadOnly,
    Low,
    Medium,
    High,
    Critical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActionKind {
    ReadFile,
    WriteFile,
    DeletePath,
    ExecuteCommand,
    StartProcess,
    StopProcess,
    HttpRequest,
    BrowserInteraction,
    ReadClipboard,
    WriteClipboard,
    CaptureScreen,
    ListenMicrophone,
    CaptureCamera,
    SendNotification,
    ReadSystemInfo,
    ControlApplication,
    InjectKeyboard,
    InjectMouse,
    GitPush,
    UploadFile,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PathOperation {
    Read,
    Write,
    Delete,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandPreview {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub network_required: bool,
    pub environment_keys: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "target_kind", rename_all = "snake_case")]
pub enum ActionTarget {
    None,
    Path {
        path: PathBuf,
        operation: PathOperation,
    },
    Command {
        program: String,
        args: Vec<String>,
        cwd: PathBuf,
        network_required: bool,
        environment_keys: Vec<String>,
    },
    Url {
        method: String,
        url: String,
        network_side_effect: bool,
    },
    Device {
        kind: String,
        scope: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActionPreview {
    pub title: String,
    pub summary: String,
    pub effects: Vec<String>,
    pub diff: Option<String>,
    pub command: Option<CommandPreview>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActionRequest {
    pub request_id: RequestId,
    pub conversation_id: Option<Uuid>,
    pub agent_id: Option<Uuid>,
    pub tool_id: String,
    pub action: ActionKind,
    pub capabilities: BTreeSet<Capability>,
    pub risk: RiskLevel,
    pub target: ActionTarget,
    pub preview: ActionPreview,
    pub requested_at: DateTime<Utc>,
}

impl ActionRequest {
    /// The digest binds a permit to the effect, not to a presentation-only
    /// request ID or timestamp.
    pub fn digest(&self) -> Result<[u8; 32], PermissionError> {
        #[derive(Serialize)]
        struct DigestView<'a> {
            tool_id: &'a str,
            action: ActionKind,
            capabilities: &'a BTreeSet<Capability>,
            risk: RiskLevel,
            target: &'a ActionTarget,
            preview: &'a ActionPreview,
        }

        let bytes = serde_json::to_vec(&DigestView {
            tool_id: &self.tool_id,
            action: self.action,
            capabilities: &self.capabilities,
            risk: self.risk,
            target: &self.target,
            preview: &self.preview,
        })
        .map_err(PermissionError::Serialization)?;
        Ok(*blake3::hash(&bytes).as_bytes())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionPolicy {
    pub workspace_roots: Vec<PathBuf>,
    pub auto_approve_read_only: bool,
    pub strict_high_risk: bool,
    pub approval_ttl_seconds: i64,
}

impl Default for PermissionPolicy {
    fn default() -> Self {
        Self {
            workspace_roots: Vec::new(),
            auto_approve_read_only: true,
            strict_high_risk: true,
            approval_ttl_seconds: 300,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalRequest {
    pub approval_id: Uuid,
    pub tool_id: String,
    pub risk: RiskLevel,
    pub preview: ActionPreview,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalRequestView {
    pub approval: ApprovalRequest,
    /// This is returned only to the trusted local UI command. It is never
    /// included in the model-facing approval result.
    pub confirmation_nonce: String,
}

#[derive(Debug, Clone)]
pub enum PermissionDecision {
    AutoApproved { permit: ExecutionPermit },
    ApprovalRequired { approval: Box<ApprovalRequest> },
    Denied { reason: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditDecision {
    AutoApproved,
    ApprovalRequested,
    UserApproved,
    UserDenied,
    Denied,
    PermitConsumed,
    PermitRejected,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditEvent {
    pub event_id: Uuid,
    pub timestamp: DateTime<Utc>,
    pub request_id: RequestId,
    pub conversation_id: Option<Uuid>,
    pub agent_id: Option<Uuid>,
    pub tool_id: String,
    pub action: ActionKind,
    pub risk: RiskLevel,
    pub target_kind: String,
    pub decision: AuditDecision,
    pub request_digest: String,
    pub detail: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ExecutionPermit {
    permit_id: Uuid,
}

#[derive(Debug)]
struct PendingApproval {
    request: ActionRequest,
    approval: ApprovalRequest,
    confirmation_nonce: String,
    confirmation_nonce_hash: Hash,
}

#[derive(Debug)]
struct PermitRecord {
    request_digest: [u8; 32],
    expires_at: DateTime<Utc>,
}

#[derive(Debug)]
pub struct PermissionBroker {
    policy: PermissionPolicy,
    workspace_roots: Vec<PathBuf>,
    pending: HashMap<Uuid, PendingApproval>,
    permits: HashMap<Uuid, PermitRecord>,
    audit_events: Vec<AuditEvent>,
}

impl PermissionBroker {
    pub fn new(policy: PermissionPolicy) -> Result<Self, PermissionError> {
        if policy.approval_ttl_seconds <= 0 {
            return Err(PermissionError::InvalidPolicy(
                "approval TTL must be positive".to_owned(),
            ));
        }

        let mut workspace_roots = Vec::with_capacity(policy.workspace_roots.len());
        for root in &policy.workspace_roots {
            if !root.is_absolute() {
                return Err(PermissionError::WorkspaceRootMustBeAbsolute(root.clone()));
            }
            if !root.exists() {
                return Err(PermissionError::WorkspaceRootMissing(root.clone()));
            }
            workspace_roots.push(fs::canonicalize(root).map_err(|source| {
                PermissionError::Canonicalization {
                    path: root.clone(),
                    source,
                }
            })?);
        }

        Ok(Self {
            policy,
            workspace_roots,
            pending: HashMap::new(),
            permits: HashMap::new(),
            audit_events: Vec::new(),
        })
    }

    pub fn policy(&self) -> &PermissionPolicy {
        &self.policy
    }

    pub fn evaluate(
        &mut self,
        request: ActionRequest,
    ) -> Result<PermissionDecision, PermissionError> {
        if request.tool_id.trim().is_empty() {
            return Ok(PermissionDecision::Denied {
                reason: "tool id is required".to_owned(),
            });
        }

        if let Err(reason) = self.validate_request(&request) {
            let reason_text = reason.to_string();
            self.record_audit(&request, AuditDecision::Denied, Some(reason_text.clone()))?;
            return Ok(PermissionDecision::Denied {
                reason: reason_text,
            });
        }

        if self.policy.auto_approve_read_only && self.is_safe_read_only(&request) {
            let permit = self.issue_permit(&request)?;
            self.record_audit(&request, AuditDecision::AutoApproved, None)?;
            return Ok(PermissionDecision::AutoApproved { permit });
        }

        let approval_id = Uuid::new_v4();
        let confirmation_nonce = Uuid::new_v4().to_string();
        let expires_at = Utc::now() + ChronoDuration::seconds(self.policy.approval_ttl_seconds);
        let approval = ApprovalRequest {
            approval_id,
            tool_id: request.tool_id.clone(),
            risk: request.risk,
            preview: request.preview.clone(),
            expires_at,
        };
        self.pending.insert(
            approval_id,
            PendingApproval {
                request: request.clone(),
                approval: approval.clone(),
                confirmation_nonce_hash: blake3::hash(confirmation_nonce.as_bytes()),
                confirmation_nonce,
            },
        );
        self.record_audit(&request, AuditDecision::ApprovalRequested, None)?;
        Ok(PermissionDecision::ApprovalRequired {
            approval: Box::new(approval),
        })
    }

    pub fn approval_for_ui(&self, approval_id: Uuid) -> Option<ApprovalRequestView> {
        self.pending
            .get(&approval_id)
            .map(|pending| ApprovalRequestView {
                approval: pending.approval.clone(),
                confirmation_nonce: pending.confirmation_nonce.clone(),
            })
    }

    pub fn approve(
        &mut self,
        approval_id: Uuid,
        confirmation_nonce: &str,
    ) -> Result<ExecutionPermit, PermissionError> {
        let pending = self
            .pending
            .get(&approval_id)
            .ok_or(PermissionError::ApprovalNotFound(approval_id))?;

        if pending.approval.expires_at <= Utc::now() {
            self.pending.remove(&approval_id);
            return Err(PermissionError::ApprovalExpired(approval_id));
        }

        if blake3::hash(confirmation_nonce.as_bytes()) != pending.confirmation_nonce_hash {
            return Err(PermissionError::InvalidConfirmation(approval_id));
        }

        let pending = self
            .pending
            .remove(&approval_id)
            .ok_or(PermissionError::ApprovalNotFound(approval_id))?;
        let request_digest = pending.request.digest()?;
        let permit = self.insert_permit(request_digest, pending.approval.expires_at);
        self.record_audit(
            &pending.request,
            AuditDecision::UserApproved,
            Some("explicit UI approval".to_owned()),
        )?;
        Ok(permit)
    }

    pub fn deny(&mut self, approval_id: Uuid) -> Result<(), PermissionError> {
        let pending = self
            .pending
            .remove(&approval_id)
            .ok_or(PermissionError::ApprovalNotFound(approval_id))?;
        self.record_audit(
            &pending.request,
            AuditDecision::UserDenied,
            Some("explicit UI denial".to_owned()),
        )?;
        Ok(())
    }

    /// Consumes the permit immediately before an executor performs its effect.
    /// The permit is removed even if the request digest is wrong, preventing a
    /// caller from probing and reusing a one-time approval.
    pub fn consume_permit(
        &mut self,
        permit: ExecutionPermit,
        request: &ActionRequest,
    ) -> Result<(), PermissionError> {
        let record = self
            .permits
            .remove(&permit.permit_id)
            .ok_or(PermissionError::PermitNotFound)?;
        // Re-check the target at the point of execution. This closes the
        // obvious evaluate/execute gap when a symlink or junction changes
        // after approval. Executors still need handle-level no-follow checks
        // for the final Windows TOCTOU boundary.
        if let Err(error) = self.validate_request(request) {
            let detail = error.to_string();
            self.record_audit(request, AuditDecision::PermitRejected, Some(detail))?;
            return Err(error);
        }
        let request_digest = request.digest()?;
        if record.request_digest != request_digest {
            self.record_audit(
                request,
                AuditDecision::PermitRejected,
                Some("digest mismatch".to_owned()),
            )?;
            return Err(PermissionError::PermitBoundToDifferentRequest);
        }
        if record.expires_at <= Utc::now() {
            self.record_audit(
                request,
                AuditDecision::PermitRejected,
                Some("permit expired".to_owned()),
            )?;
            return Err(PermissionError::PermitExpired);
        }
        self.record_audit(request, AuditDecision::PermitConsumed, None)?;
        Ok(())
    }

    pub fn take_audit_events(&mut self) -> Vec<AuditEvent> {
        std::mem::take(&mut self.audit_events)
    }

    fn validate_request(&self, request: &ActionRequest) -> Result<(), PermissionError> {
        let expected = expected_capability(request.action);
        if !request.capabilities.contains(&expected) {
            return Err(PermissionError::MissingCapability(expected));
        }
        if !action_target_is_consistent(request.action, &request.target) {
            return Err(PermissionError::InvalidTarget(
                "action and target kinds do not match".to_owned(),
            ));
        }

        match &request.target {
            ActionTarget::None => Ok(()),
            ActionTarget::Path { path, operation } => {
                let normalized = canonicalize_candidate(path)?;
                if !self.is_within_workspace(&normalized) {
                    return Err(PermissionError::OutsideWorkspace(normalized));
                }
                let operation_capability = match operation {
                    PathOperation::Read => Capability::FilesystemRead,
                    PathOperation::Write => Capability::FilesystemWrite,
                    PathOperation::Delete => Capability::FilesystemDelete,
                };
                if !request.capabilities.contains(&operation_capability) {
                    return Err(PermissionError::MissingCapability(operation_capability));
                }
                Ok(())
            }
            ActionTarget::Command { cwd, program, .. } => {
                if program.trim().is_empty() {
                    return Err(PermissionError::InvalidTarget(
                        "command program is empty".to_owned(),
                    ));
                }
                let normalized = canonicalize_candidate(cwd)?;
                if !self.is_within_workspace(&normalized) {
                    return Err(PermissionError::OutsideWorkspace(normalized));
                }
                Ok(())
            }
            ActionTarget::Url { method, url, .. } => {
                let parsed = url::Url::parse(url)
                    .map_err(|_| PermissionError::InvalidTarget("URL is malformed".to_owned()))?;
                if !matches!(parsed.scheme(), "http" | "https") {
                    return Err(PermissionError::InvalidTarget(
                        "only http and https URLs are allowed".to_owned(),
                    ));
                }
                if method.trim().is_empty() {
                    return Err(PermissionError::InvalidTarget(
                        "HTTP method is empty".to_owned(),
                    ));
                }
                Ok(())
            }
            ActionTarget::Device { kind, scope } => {
                if kind.trim().is_empty() || scope.trim().is_empty() {
                    return Err(PermissionError::InvalidTarget(
                        "device kind and scope are required".to_owned(),
                    ));
                }
                Ok(())
            }
        }
    }

    fn is_safe_read_only(&self, request: &ActionRequest) -> bool {
        if request.risk != RiskLevel::ReadOnly {
            return false;
        }

        match request.action {
            ActionKind::ReadFile => {
                request
                    .capabilities
                    .iter()
                    .all(|cap| *cap == Capability::FilesystemRead)
                    && matches!(
                        &request.target,
                        ActionTarget::Path {
                            operation: PathOperation::Read,
                            ..
                        }
                    )
            }
            ActionKind::ReadSystemInfo => {
                request
                    .capabilities
                    .iter()
                    .all(|cap| *cap == Capability::SystemInfo)
                    && matches!(&request.target, ActionTarget::None)
            }
            _ => false,
        }
    }

    fn is_within_workspace(&self, path: &Path) -> bool {
        self.workspace_roots
            .iter()
            .any(|root| path == root || path.starts_with(root))
    }

    fn issue_permit(
        &mut self,
        request: &ActionRequest,
    ) -> Result<ExecutionPermit, PermissionError> {
        let digest = request.digest()?;
        Ok(self.insert_permit(
            digest,
            Utc::now() + ChronoDuration::seconds(self.policy.approval_ttl_seconds),
        ))
    }

    fn insert_permit(
        &mut self,
        request_digest: [u8; 32],
        expires_at: DateTime<Utc>,
    ) -> ExecutionPermit {
        let permit_id = Uuid::new_v4();
        self.permits.insert(
            permit_id,
            PermitRecord {
                request_digest,
                expires_at,
            },
        );
        ExecutionPermit { permit_id }
    }

    fn record_audit(
        &mut self,
        request: &ActionRequest,
        decision: AuditDecision,
        detail: Option<String>,
    ) -> Result<(), PermissionError> {
        let request_digest = request.digest()?;
        self.audit_events.push(AuditEvent {
            event_id: Uuid::new_v4(),
            timestamp: Utc::now(),
            request_id: request.request_id,
            conversation_id: request.conversation_id,
            agent_id: request.agent_id,
            tool_id: request.tool_id.clone(),
            action: request.action,
            risk: request.risk,
            target_kind: target_kind(&request.target).to_owned(),
            decision,
            request_digest: hex_encode(&request_digest),
            detail,
        });
        Ok(())
    }
}

fn expected_capability(action: ActionKind) -> Capability {
    match action {
        ActionKind::ReadFile => Capability::FilesystemRead,
        ActionKind::WriteFile => Capability::FilesystemWrite,
        ActionKind::DeletePath => Capability::FilesystemDelete,
        ActionKind::ExecuteCommand => Capability::ShellExecute,
        ActionKind::StartProcess => Capability::ProcessStart,
        ActionKind::StopProcess => Capability::ProcessStop,
        ActionKind::HttpRequest => Capability::NetworkHttp,
        ActionKind::BrowserInteraction => Capability::BrowserInteract,
        ActionKind::ReadClipboard => Capability::ClipboardRead,
        ActionKind::WriteClipboard => Capability::ClipboardWrite,
        ActionKind::CaptureScreen => Capability::ScreenCapture,
        ActionKind::ListenMicrophone => Capability::MicrophoneListen,
        ActionKind::CaptureCamera => Capability::CameraCapture,
        ActionKind::SendNotification => Capability::NotificationsSend,
        ActionKind::ReadSystemInfo => Capability::SystemInfo,
        ActionKind::ControlApplication => Capability::ApplicationControl,
        ActionKind::InjectKeyboard => Capability::KeyboardInject,
        ActionKind::InjectMouse => Capability::MouseInject,
        ActionKind::GitPush => Capability::GitPush,
        ActionKind::UploadFile => Capability::FileUpload,
    }
}

fn action_target_is_consistent(action: ActionKind, target: &ActionTarget) -> bool {
    match action {
        ActionKind::ReadFile => matches!(
            target,
            ActionTarget::Path {
                operation: PathOperation::Read,
                ..
            }
        ),
        ActionKind::WriteFile => matches!(
            target,
            ActionTarget::Path {
                operation: PathOperation::Write,
                ..
            }
        ),
        ActionKind::DeletePath => matches!(
            target,
            ActionTarget::Path {
                operation: PathOperation::Delete,
                ..
            }
        ),
        ActionKind::ExecuteCommand | ActionKind::GitPush => {
            matches!(target, ActionTarget::Command { .. })
        }
        ActionKind::HttpRequest | ActionKind::BrowserInteraction | ActionKind::UploadFile => {
            matches!(target, ActionTarget::Url { .. })
        }
        ActionKind::StartProcess
        | ActionKind::StopProcess
        | ActionKind::CaptureScreen
        | ActionKind::ListenMicrophone
        | ActionKind::CaptureCamera
        | ActionKind::ControlApplication
        | ActionKind::InjectKeyboard
        | ActionKind::InjectMouse => matches!(target, ActionTarget::Device { .. }),
        ActionKind::ReadClipboard
        | ActionKind::WriteClipboard
        | ActionKind::SendNotification
        | ActionKind::ReadSystemInfo => matches!(target, ActionTarget::None),
    }
}

fn target_kind(target: &ActionTarget) -> &'static str {
    match target {
        ActionTarget::None => "none",
        ActionTarget::Path { .. } => "path",
        ActionTarget::Command { .. } => "command",
        ActionTarget::Url { .. } => "url",
        ActionTarget::Device { .. } => "device",
    }
}

fn canonicalize_candidate(path: &Path) -> Result<PathBuf, PermissionError> {
    if !path.is_absolute() {
        return Err(PermissionError::PathMustBeAbsolute(path.to_path_buf()));
    }
    if path
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(PermissionError::PathTraversal(path.to_path_buf()));
    }

    if path.exists() {
        return fs::canonicalize(path).map_err(|source| PermissionError::Canonicalization {
            path: path.to_path_buf(),
            source,
        });
    }

    let mut missing: Vec<OsString> = Vec::new();
    let mut cursor = path.to_path_buf();
    while !cursor.exists() {
        let component = cursor
            .file_name()
            .ok_or_else(|| PermissionError::InvalidTarget("path has no filename".to_owned()))?;
        missing.push(component.to_os_string());
        if !cursor.pop() {
            return Err(PermissionError::PathDoesNotResolve(path.to_path_buf()));
        }
    }

    let mut normalized =
        fs::canonicalize(&cursor).map_err(|source| PermissionError::Canonicalization {
            path: cursor.clone(),
            source,
        })?;
    while let Some(component) = missing.pop() {
        normalized.push(component);
    }
    Ok(normalized)
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

#[derive(Debug, Error)]
pub enum PermissionError {
    #[error("invalid permission policy: {0}")]
    InvalidPolicy(String),
    #[error("workspace root must be absolute: {0}")]
    WorkspaceRootMustBeAbsolute(PathBuf),
    #[error("workspace root does not exist: {0}")]
    WorkspaceRootMissing(PathBuf),
    #[error("path must be absolute: {0}")]
    PathMustBeAbsolute(PathBuf),
    #[error("parent traversal is not allowed: {0}")]
    PathTraversal(PathBuf),
    #[error("path does not resolve to an existing parent: {0}")]
    PathDoesNotResolve(PathBuf),
    #[error("path is outside the selected workspace: {0}")]
    OutsideWorkspace(PathBuf),
    #[error("missing capability: {0:?}")]
    MissingCapability(Capability),
    #[error("invalid action target: {0}")]
    InvalidTarget(String),
    #[error("failed to canonicalize {path}: {source}")]
    Canonicalization {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("serialization failed: {0}")]
    Serialization(serde_json::Error),
    #[error("approval not found: {0}")]
    ApprovalNotFound(Uuid),
    #[error("approval expired: {0}")]
    ApprovalExpired(Uuid),
    #[error("approval confirmation is invalid: {0}")]
    InvalidConfirmation(Uuid),
    #[error("permit not found or already consumed")]
    PermitNotFound,
    #[error("permit is bound to a different action request")]
    PermitBoundToDifferentRequest,
    #[error("permit expired")]
    PermitExpired,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn read_request(path: PathBuf) -> ActionRequest {
        ActionRequest {
            request_id: RequestId::new(),
            conversation_id: None,
            agent_id: None,
            tool_id: "builtin.files.read".to_owned(),
            action: ActionKind::ReadFile,
            capabilities: BTreeSet::from([Capability::FilesystemRead]),
            risk: RiskLevel::ReadOnly,
            target: ActionTarget::Path {
                path,
                operation: PathOperation::Read,
            },
            preview: ActionPreview {
                title: "Read file".to_owned(),
                summary: "Read a workspace file".to_owned(),
                effects: vec!["No file changes".to_owned()],
                diff: None,
                command: None,
            },
            requested_at: Utc::now(),
        }
    }

    fn command_request(root: PathBuf) -> ActionRequest {
        ActionRequest {
            request_id: RequestId::new(),
            conversation_id: None,
            agent_id: None,
            tool_id: "builtin.shell".to_owned(),
            action: ActionKind::ExecuteCommand,
            capabilities: BTreeSet::from([Capability::ShellExecute]),
            risk: RiskLevel::High,
            target: ActionTarget::Command {
                program: "git".to_owned(),
                args: vec!["status".to_owned()],
                cwd: root.clone(),
                network_required: false,
                environment_keys: Vec::new(),
            },
            preview: ActionPreview {
                title: "Run command".to_owned(),
                summary: "Run git status in the workspace".to_owned(),
                effects: vec!["Reads repository state".to_owned()],
                diff: None,
                command: Some(CommandPreview {
                    program: "git".to_owned(),
                    args: vec!["status".to_owned()],
                    cwd: root,
                    network_required: false,
                    environment_keys: Vec::new(),
                }),
            },
            requested_at: Utc::now(),
        }
    }

    #[test]
    fn read_inside_workspace_can_be_auto_approved_and_consumed_once() {
        let directory = tempfile::tempdir().expect("temp directory");
        let file = directory.path().join("README.md");
        fs::write(&file, "safe").expect("file writes");
        let mut broker = PermissionBroker::new(PermissionPolicy {
            workspace_roots: vec![directory.path().to_path_buf()],
            ..PermissionPolicy::default()
        })
        .expect("broker creates");

        let request = read_request(file);
        let decision = broker.evaluate(request.clone()).expect("evaluate");
        let permit = match decision {
            PermissionDecision::AutoApproved { permit } => permit,
            other => panic!("expected auto approval, got {other:?}"),
        };
        broker.consume_permit(permit, &request).expect("consume");
        assert!(matches!(
            broker.consume_permit(
                ExecutionPermit {
                    permit_id: Uuid::new_v4()
                },
                &request
            ),
            Err(PermissionError::PermitNotFound)
        ));
    }

    #[test]
    fn shell_never_auto_executes() {
        let directory = tempfile::tempdir().expect("temp directory");
        let mut broker = PermissionBroker::new(PermissionPolicy {
            workspace_roots: vec![directory.path().to_path_buf()],
            ..PermissionPolicy::default()
        })
        .expect("broker creates");
        let decision = broker
            .evaluate(command_request(directory.path().to_path_buf()))
            .expect("evaluate");
        assert!(matches!(
            decision,
            PermissionDecision::ApprovalRequired { .. }
        ));
    }

    #[test]
    fn outside_workspace_is_denied() {
        let workspace = tempfile::tempdir().expect("workspace");
        let outside = tempfile::tempdir().expect("outside");
        let file = outside.path().join("private.txt");
        fs::write(&file, "private").expect("file writes");
        let mut broker = PermissionBroker::new(PermissionPolicy {
            workspace_roots: vec![workspace.path().to_path_buf()],
            ..PermissionPolicy::default()
        })
        .expect("broker creates");
        let decision = broker.evaluate(read_request(file)).expect("evaluate");
        assert!(matches!(decision, PermissionDecision::Denied { .. }));
    }

    #[test]
    fn approval_is_bound_to_exact_request_and_nonce() {
        let directory = tempfile::tempdir().expect("temp directory");
        let mut broker = PermissionBroker::new(PermissionPolicy {
            workspace_roots: vec![directory.path().to_path_buf()],
            ..PermissionPolicy::default()
        })
        .expect("broker creates");
        let request = command_request(directory.path().to_path_buf());
        let approval_id = match broker.evaluate(request.clone()).expect("evaluate") {
            PermissionDecision::ApprovalRequired { approval } => approval.approval_id,
            other => panic!("expected approval, got {other:?}"),
        };
        let view = broker.approval_for_ui(approval_id).expect("UI view");
        assert!(matches!(
            broker.approve(approval_id, "wrong"),
            Err(PermissionError::InvalidConfirmation(_))
        ));
        let permit = broker
            .approve(approval_id, &view.confirmation_nonce)
            .expect("approval");
        broker.consume_permit(permit, &request).expect("consume");
        assert!(matches!(
            broker.approve(approval_id, &view.confirmation_nonce),
            Err(PermissionError::ApprovalNotFound(_))
        ));
    }

    #[test]
    fn parent_traversal_is_rejected_even_when_it_would_normalize_inside() {
        let workspace = tempfile::tempdir().expect("workspace");
        let mut broker = PermissionBroker::new(PermissionPolicy {
            workspace_roots: vec![workspace.path().to_path_buf()],
            ..PermissionPolicy::default()
        })
        .expect("broker creates");
        let request = read_request(workspace.path().join("child").join("..").join("file"));
        let decision = broker.evaluate(request).expect("evaluate");
        assert!(matches!(decision, PermissionDecision::Denied { .. }));
    }

    #[cfg(unix)]
    #[test]
    fn symlink_escape_is_rejected() {
        let workspace = tempfile::tempdir().expect("workspace");
        let outside = tempfile::tempdir().expect("outside");
        let outside_file = outside.path().join("private.txt");
        fs::write(&outside_file, "private").expect("file writes");
        let link = workspace.path().join("link.txt");
        std::os::unix::fs::symlink(&outside_file, &link).expect("symlink creates");
        let mut broker = PermissionBroker::new(PermissionPolicy {
            workspace_roots: vec![workspace.path().to_path_buf()],
            ..PermissionPolicy::default()
        })
        .expect("broker creates");

        let decision = broker.evaluate(read_request(link)).expect("evaluate");
        assert!(matches!(decision, PermissionDecision::Denied { .. }));
    }

    #[cfg(unix)]
    #[test]
    fn permit_revalidates_a_target_before_consumption() {
        let workspace = tempfile::tempdir().expect("workspace");
        let outside = tempfile::tempdir().expect("outside");
        let file = workspace.path().join("mutable.txt");
        let outside_file = outside.path().join("private.txt");
        fs::write(&file, "workspace").expect("workspace file writes");
        fs::write(&outside_file, "private").expect("outside file writes");
        let mut broker = PermissionBroker::new(PermissionPolicy {
            workspace_roots: vec![workspace.path().to_path_buf()],
            ..PermissionPolicy::default()
        })
        .expect("broker creates");
        let request = read_request(file.clone());
        let permit = match broker.evaluate(request.clone()).expect("evaluate") {
            PermissionDecision::AutoApproved { permit } => permit,
            other => panic!("expected auto approval, got {other:?}"),
        };
        fs::remove_file(&file).expect("workspace file removes");
        std::os::unix::fs::symlink(&outside_file, &file).expect("replacement symlink creates");

        assert!(matches!(
            broker.consume_permit(permit, &request),
            Err(PermissionError::OutsideWorkspace(_))
        ));
    }
}
