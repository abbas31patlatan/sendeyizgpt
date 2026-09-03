# ADR 0001: keep native runtimes outside the desktop process

Status: accepted for Milestone 0

## Context

Inference runtimes and device drivers are native code with a larger crash and
supply-chain surface than the Rust orchestration core. AMD Vulkan support also
depends on driver/runtime combinations that cannot be treated as a stable UI
dependency.

## Decision

The Tauri desktop process owns UI commands, trusted policy, worker lifecycle
and persistence coordination. Inference, agent planning, tool execution,
browser, audio and vision run behind typed, versioned local IPC. The first
inference adapter is a supervised llama.cpp worker. Windows production IPC is a
per-launch named pipe with a current-user ACL and HMAC-authenticated frames.

## Consequences

Positive:

- Backend crashes can be recovered without discarding the desktop shell.
- Runtime versions and accelerator adapters can evolve independently.
- The core never needs to link C/C++ inference code.

Costs:

- Worker startup, streaming and cancellation need explicit lifecycle code.
- Packaging is more complex because runtime binaries are separate artifacts.
- IPC schemas and compatibility tests become part of the release contract.

