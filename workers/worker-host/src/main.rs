//! Minimal worker lifecycle probe for Milestone 0.
//!
//! It intentionally has no host-side capabilities and does not execute a
//! worker protocol yet. The production supervisor will replace stdout with a
//! named-pipe/stdio transport and require `aegis-ipc` authentication before it
//! accepts any worker message.

use aegis_protocol::{Frame, FrameKind, JsonRequest};
use std::error::Error;

fn main() -> Result<(), Box<dyn Error>> {
    let worker_kind = std::env::args()
        .skip(1)
        .find_map(|argument| argument.strip_prefix("--kind=").map(str::to_owned))
        .unwrap_or_else(|| "unknown".to_owned());

    let body = JsonRequest {
        operation: "worker_probe".to_owned(),
        payload: serde_json::json!({
            "worker_kind": worker_kind,
            "capabilities": [],
            "authenticated": false,
            "production_transport": "not_enabled"
        }),
    };
    let frame = Frame::new(FrameKind::Event, "worker.lifecycle.probe", body);
    println!("{}", String::from_utf8(frame.to_json_bytes()?)?);
    Ok(())
}
