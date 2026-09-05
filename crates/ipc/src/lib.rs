//! Authentication and framing helpers for local worker IPC.
//!
//! Transport implementations intentionally live outside this crate. The
//! production Windows transport will use a current-user named pipe; this
//! crate only knows how to authenticate a typed protocol frame.

use aegis_protocol::{Frame, ProtocolError};
use getrandom::fill as fill_random;
use hmac::{Hmac, Mac};
use serde::Serialize;
use sha2::Sha256;
use std::sync::atomic::{AtomicU64, Ordering};
use thiserror::Error;

type HmacSha256 = Hmac<Sha256>;

const SESSION_KEY_BYTES: usize = 32;

#[derive(Clone)]
pub struct SessionSecret([u8; SESSION_KEY_BYTES]);

impl SessionSecret {
    pub fn generate() -> Result<Self, IpcError> {
        let mut bytes = [0_u8; SESSION_KEY_BYTES];
        fill_random(&mut bytes).map_err(IpcError::Randomness)?;
        Ok(Self(bytes))
    }

    pub fn from_bytes(bytes: [u8; SESSION_KEY_BYTES]) -> Self {
        Self(bytes)
    }
}

impl std::fmt::Debug for SessionSecret {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("SessionSecret(REDACTED)")
    }
}

pub struct FrameAuthenticator {
    secret: SessionSecret,
}

#[derive(Debug, Default)]
pub struct ReplayGuard {
    highest_received: AtomicU64,
}

impl ReplayGuard {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn verify_and_accept<T: Serialize>(
        &self,
        authenticator: &FrameAuthenticator,
        frame: &Frame<T>,
    ) -> Result<(), IpcError> {
        authenticator.verify(frame)?;
        let sequence = frame.header.sequence;
        if sequence == 0 {
            return Err(IpcError::ReplayRejected);
        }

        let mut highest = self.highest_received.load(Ordering::Acquire);
        loop {
            if sequence <= highest {
                return Err(IpcError::ReplayRejected);
            }
            match self.highest_received.compare_exchange(
                highest,
                sequence,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return Ok(()),
                Err(actual) => highest = actual,
            }
        }
    }
}

impl FrameAuthenticator {
    pub fn new(secret: SessionSecret) -> Self {
        Self { secret }
    }

    pub fn seal<T: Serialize>(&self, frame: &mut Frame<T>) -> Result<(), IpcError> {
        let bytes = frame.signing_bytes().map_err(IpcError::Protocol)?;
        let mut mac = HmacSha256::new_from_slice(&self.secret.0)
            .map_err(|_| IpcError::InvalidSecretLength)?;
        mac.update(&bytes);
        frame.auth_tag = Some(hex_encode(&mac.finalize().into_bytes()));
        Ok(())
    }

    pub fn verify<T: Serialize>(&self, frame: &Frame<T>) -> Result<(), IpcError> {
        let tag = frame.auth_tag.as_deref().ok_or(IpcError::MissingAuthTag)?;
        let tag_bytes = hex_decode(tag).ok_or(IpcError::MalformedAuthTag)?;
        let bytes = frame.signing_bytes().map_err(IpcError::Protocol)?;
        let mut mac = HmacSha256::new_from_slice(&self.secret.0)
            .map_err(|_| IpcError::InvalidSecretLength)?;
        mac.update(&bytes);
        mac.verify_slice(&tag_bytes)
            .map_err(|_| IpcError::AuthenticationFailed)
    }
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

fn hex_decode(value: &str) -> Option<Vec<u8>> {
    if value.len() != 64 {
        return None;
    }

    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| Some((hex_nibble(pair[0])? << 4) | hex_nibble(pair[1])?))
        .collect()
}

fn hex_nibble(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

#[derive(Debug, Error)]
pub enum IpcError {
    #[error("OS randomness unavailable: {0}")]
    Randomness(getrandom::Error),
    #[error("protocol error: {0}")]
    Protocol(ProtocolError),
    #[error("session secret has an invalid length")]
    InvalidSecretLength,
    #[error("IPC frame has no authentication tag")]
    MissingAuthTag,
    #[error("IPC frame authentication tag is malformed")]
    MalformedAuthTag,
    #[error("IPC frame authentication failed")]
    AuthenticationFailed,
    #[error("IPC frame sequence was replayed or missing")]
    ReplayRejected,
}

#[cfg(test)]
mod tests {
    use super::*;
    use aegis_protocol::{Frame, FrameKind, JsonRequest};

    #[test]
    fn authenticates_and_rejects_tampering() {
        let secret = SessionSecret::from_bytes([7_u8; SESSION_KEY_BYTES]);
        let authenticator = FrameAuthenticator::new(secret);
        let mut frame = Frame::new(
            FrameKind::Request,
            "health.check",
            JsonRequest {
                operation: "ping".to_owned(),
                payload: serde_json::json!({"ok": true}),
            },
        );
        authenticator.seal(&mut frame).expect("frame seals");
        authenticator.verify(&frame).expect("frame verifies");

        frame.body.operation = "changed".to_owned();
        assert!(matches!(
            authenticator.verify(&frame),
            Err(IpcError::AuthenticationFailed)
        ));
    }

    #[test]
    fn never_formats_secret_in_debug() {
        let secret = SessionSecret::from_bytes([1_u8; SESSION_KEY_BYTES]);
        assert_eq!(format!("{secret:?}"), "SessionSecret(REDACTED)");
    }

    #[test]
    fn replay_guard_accepts_each_sequence_once() {
        let secret = SessionSecret::from_bytes([3_u8; SESSION_KEY_BYTES]);
        let authenticator = FrameAuthenticator::new(secret);
        let guard = ReplayGuard::new();
        let mut frame = Frame::new(
            FrameKind::Event,
            "health.check",
            JsonRequest {
                operation: "ping".to_owned(),
                payload: serde_json::json!({}),
            },
        )
        .with_sequence(1);
        authenticator.seal(&mut frame).expect("frame seals");
        guard
            .verify_and_accept(&authenticator, &frame)
            .expect("first frame accepted");
        assert!(matches!(
            guard.verify_and_accept(&authenticator, &frame),
            Err(IpcError::ReplayRejected)
        ));
    }
}
