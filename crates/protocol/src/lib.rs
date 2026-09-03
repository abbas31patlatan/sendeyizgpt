//! Versioned, transport-neutral wire primitives shared by the desktop core
//! and isolated workers. Domain crates supply the typed `Frame<T>` body.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fmt::{Display, Formatter};
use std::str::FromStr;
use thiserror::Error;
use uuid::Uuid;

pub const PROTOCOL_MAJOR: u16 = 1;
pub const PROTOCOL_MINOR: u16 = 0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtocolVersion {
    pub major: u16,
    pub minor: u16,
}

impl ProtocolVersion {
    pub const CURRENT: Self = Self {
        major: PROTOCOL_MAJOR,
        minor: PROTOCOL_MINOR,
    };

    pub const fn is_compatible_with(self, peer: Self) -> bool {
        self.major == peer.major && peer.minor <= self.minor
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RequestId(Uuid);

impl RequestId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    pub const fn from_uuid(value: Uuid) -> Self {
        Self(value)
    }

    pub const fn as_uuid(self) -> Uuid {
        self.0
    }
}

impl Default for RequestId {
    fn default() -> Self {
        Self::new()
    }
}

impl Display for RequestId {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(formatter)
    }
}

impl FromStr for RequestId {
    type Err = uuid::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Ok(Self(Uuid::parse_str(value)?))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkerKind {
    Agent,
    Inference,
    Tools,
    Browser,
    Audio,
    Vision,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FrameKind {
    Request,
    Response,
    Event,
    Cancel,
    Handshake,
    HandshakeAck,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FrameHeader {
    pub protocol: ProtocolVersion,
    pub request_id: RequestId,
    /// Monotonic per-direction sequence. The IPC layer rejects replayed
    /// sequence numbers after authenticating the frame.
    pub sequence: u64,
    pub kind: FrameKind,
    pub message_type: String,
}

impl FrameHeader {
    pub fn new(kind: FrameKind, message_type: impl Into<String>) -> Self {
        Self {
            protocol: ProtocolVersion::CURRENT,
            request_id: RequestId::new(),
            sequence: 0,
            kind,
            message_type: message_type.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Frame<T> {
    pub header: FrameHeader,
    pub body: T,
    /// HMAC encoded by the IPC layer. It is excluded from signing bytes.
    pub auth_tag: Option<String>,
}

impl<T> Frame<T> {
    pub fn new(kind: FrameKind, message_type: impl Into<String>, body: T) -> Self {
        Self {
            header: FrameHeader::new(kind, message_type),
            body,
            auth_tag: None,
        }
    }

    pub fn with_sequence(mut self, sequence: u64) -> Self {
        self.header.sequence = sequence;
        self
    }

    pub fn map_body<U>(self, map: impl FnOnce(T) -> U) -> Frame<U> {
        Frame {
            header: self.header,
            body: map(self.body),
            auth_tag: self.auth_tag,
        }
    }
}

impl<T: Serialize> Frame<T> {
    /// Serializes only the authenticated header and payload. The auth tag is
    /// intentionally omitted so signing is deterministic and non-recursive.
    pub fn signing_bytes(&self) -> Result<Vec<u8>, ProtocolError> {
        #[derive(Serialize)]
        struct SigningView<'a, T> {
            header: &'a FrameHeader,
            body: &'a T,
        }

        serde_json::to_vec(&SigningView {
            header: &self.header,
            body: &self.body,
        })
        .map_err(ProtocolError::Serialization)
    }

    pub fn to_json_bytes(&self) -> Result<Vec<u8>, ProtocolError> {
        serde_json::to_vec(self).map_err(ProtocolError::Serialization)
    }
}

impl<T: DeserializeOwned> Frame<T> {
    pub fn from_json_bytes(bytes: &[u8]) -> Result<Self, ProtocolError> {
        serde_json::from_slice(bytes).map_err(ProtocolError::Deserialization)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HandshakeHello {
    pub worker_kind: WorkerKind,
    pub worker_instance: String,
    pub protocol: ProtocolVersion,
    pub challenge: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HandshakeAck {
    pub accepted: bool,
    pub protocol: ProtocolVersion,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CancelRequest {
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorBody {
    pub code: String,
    pub message: String,
    pub retryable: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JsonRequest {
    pub operation: String,
    pub payload: Value,
}

#[derive(Debug, Error)]
pub enum ProtocolError {
    #[error("protocol serialization failed: {0}")]
    Serialization(serde_json::Error),
    #[error("protocol deserialization failed: {0}")]
    Deserialization(serde_json::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_major_version_mismatch() {
        assert!(!ProtocolVersion::CURRENT.is_compatible_with(ProtocolVersion {
            major: PROTOCOL_MAJOR + 1,
            minor: 0,
        }));
    }

    #[test]
    fn round_trips_typed_frame() {
        let frame = Frame::new(
            FrameKind::Request,
            "health.check",
            JsonRequest {
                operation: "ping".to_owned(),
                payload: serde_json::json!({"seq": 1}),
            },
        );
        let bytes = frame.to_json_bytes().expect("frame serializes");
        let restored = Frame::<JsonRequest>::from_json_bytes(&bytes).expect("frame parses");
        assert_eq!(restored.body.operation, "ping");
        assert_eq!(restored.header.message_type, "health.check");
    }
}
