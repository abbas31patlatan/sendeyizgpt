# ADR 0001: keep native runtimes outside the desktop process

Status: accepted and extended for Milestone 2b

## Context

Inference runtimes and device drivers are native code with a larger crash and
supply-chain surface than the Rust orchestration core. AMD Vulkan support also
depends on driver/runtime combinations that cannot be treated as a stable UI
dependency.

## Decision

The Tauri desktop process owns UI commands, trusted policy, worker lifecycle
and persistence coordination. The native llama.cpp adapter is a supervised
`llama-server` process outside that address space: it receives a revalidated GGUF
path and profile via an argument vector, binds to a random `127.0.0.1` port, and
is considered loaded only after `/health` succeeds. The existing OpenAI-compatible
client then uses its `/v1` endpoint for streaming and cancellation. Inference,
agent planning, tool execution, browser, audio and vision remain replaceable
worker capabilities; agent/tool workers use typed, versioned authenticated IPC
with a per-launch Windows named pipe, current-user ACL and HMAC frames.

## Consequences

Positive:

- Backend crashes can be recovered without discarding the desktop shell.
- Runtime versions and accelerator adapters can evolve independently.
- The core never needs to link C/C++ inference code.

Costs:

- Worker startup, streaming and cancellation need explicit lifecycle code.
- Packaging is more complex because runtime binaries are separate artifacts and
  the pinned CPU build must be reproducible.
- Loopback HTTP is intentionally limited to the native server's local API; future
  agent/tool workers still require authenticated IPC schemas and compatibility tests.

