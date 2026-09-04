# Aegis AI

Aegis AI is a local-first Windows AI work environment. Its core boundary is
deliberately small: models, agents, tools, providers, events, memory and
workspaces are separate capabilities, while every computer-side effect is
mediated by a native Permission Broker.

## Current status

Milestone 0 foundation, M1a local chat, M1b persistence/workspace, M2a model catalog/preflight and M2b supervised native llama.cpp runtime are in place:

- Tauri 2 + React/TypeScript desktop shell
- Rust workspace with typed protocol, IPC authentication helpers, inference and agent contracts
- Permission Broker with workspace path checks, approval records and one-time execution permits
- SQLite migration with foreign keys, WAL and core domain tables
- OpenAI-compatible provider adapter with validation, SSE streaming, cancellation and retry classification
- Responsive bilingual chat/settings UI with Turkish/English copy, system/dark/light themes, searchable conversation history and explicit unavailable surfaces
- Frame-batched streaming updates and debounced persistence to keep long responses responsive
- SQLite-backed conversation/message repository with schema migration, reasoning preservation and cascade-safe deletion
- Native app-data database initialization with a localStorage fallback for Vite preview/private browsing
- Read-only workspace path validation and a durable, explicitly scoped workspace registry
- Provider health diagnostics with measured latency, live model catalog and model-card selection
- Bounded, metadata-only GGUF scanner with canonical roots, symlink skipping, deterministic traversal and per-file issue reporting
- SQLite-backed model libraries, real local inventory and Eco/Balanced/Performance profiles with advisory memory preflight
- Supervised native llama.cpp `llama-server` lifecycle: GGUF revalidation, real tensor-load health gate, loopback endpoint and clean unload
- Bilingual native runtime controls with bundled pinned CPU runtime support and explicit external GPU-runtime path selection
- Per-provider system prompt, conversation rename/Markdown export and strict bilingual UX
- Architecture, security, plugin and development documentation

Native llama.cpp tensor loading and generation are now wired through a supervised
`llama-server` process. Aegis re-inspects the cataloged GGUF, launches a real native
executable on a random loopback port and marks the model ready only after the
server's `/health` gate succeeds. The Windows package builds a pinned, static CPU
runtime from upstream; a compatible external GPU build can be selected by PATH or
an explicit executable path. Browser, audio, vision and real tool execution remain
separate future worker capabilities.

## Prerequisites

Development on Windows requires:

- Windows 10/11 x64
- Visual Studio C++ Build Tools
- Rust stable with the `x86_64-pc-windows-msvc` target
- Node.js 20+ and npm
- Tauri CLI (`cargo install tauri-cli --locked`)
- Git and CMake with the Visual Studio C++ workload (needed to build the pinned runtime)
- Vulkan runtime/driver only when using a compatible external GPU build; the bundled release runtime is CPU-native

## Commands

```powershell
npm install --prefix frontend
npm install --prefix apps/desktop
npm run typecheck
npm run build
cargo test --workspace
npm run desktop:dev
```

On a Windows development machine, the complete verification and installer
build can be run with:

```powershell
powershell -ExecutionPolicy Bypass -File .\scripts\build-llama-runtime.ps1
powershell -ExecutionPolicy Bypass -File .\scripts\build-windows.ps1
```

The current scratch environment does not include Rust/Cargo, so the Rust
commands are intentionally left for a Windows developer environment or CI.

## Functional local chat

The M1 chat slice is now wired end to end through an OpenAI-compatible provider.
It works with local servers such as Ollama and LM Studio, as well as compatible
self-hosted or remote endpoints. The desktop process owns the HTTP request and
SSE parsing; the browser UI never receives provider credentials from logs or
local storage.

1. Start a compatible server and make one model available.
2. Run the desktop app with `npm run desktop:dev`.
3. Open **Model library**, set the provider base URL and model, then choose
   **Check connection**.
4. Return to **Chats** and send a message. Responses stream incrementally and
   can be cancelled from the composer or **Stop everything**.

Default local endpoint: `http://127.0.0.1:11434/v1` (Ollama). LM Studio
normally uses `http://127.0.0.1:1234/v1`. Quick provider cards can apply
either local endpoint. The optional API key is held only in memory for the
current session and is never persisted by the frontend or SQLite.

### Native llama.cpp local chat

For a model-file workflow, use the **Model library** without running Ollama or
LM Studio:

1. Register a directory and scan it; only bounded GGUF metadata is indexed.
2. Select a model, choose an Eco/Balanced/Performance profile and review preflight.
3. Choose the bundled runtime or enter a compatible external `llama-server` path,
   then select **Load tensors**.
4. Wait for the status to become **Ready**. The desktop routes chat to the
   supervised loopback `/v1` endpoint and uses the same streaming/reasoning/cancel
   transport as the other OpenAI-compatible providers.
5. Select **Unload** when finished; the native process is terminated and its state
   is cleared.

The packaged Windows runtime is deliberately CPU-native and reproducible. GPU
acceleration remains available through a user-selected compatible build so a
driver/backend choice is never silently guessed. When the native server exposes
its Prometheus metrics endpoint, the runtime panel shows bounded prompt and
generation throughput, token counters and queue state; unavailable telemetry is
shown as unavailable rather than fabricated.

## Interface and performance

The interface follows the operating-system language on first launch and can be
switched between Turkish and English from the top bar. System, dark and light
themes are persisted as non-secret preferences. Recent conversations can be
searched with `Ctrl+K`, created with `Ctrl+N`, copied message-by-message and
deleted with confirmation.

Streaming token and reasoning events are coalesced into animation-frame updates
before React state is changed. Conversation persistence is paused while a response
is streaming and flushed after terminal events, so long responses do not rewrite
the full SQLite history per token. The native repository uses one transaction per
conversation snapshot and keeps browser localStorage as a preview fallback.

## Documents

- [ARCHITECTURE.md](ARCHITECTURE.md)
- [SECURITY.md](SECURITY.md)
- [PLUGIN_API.md](PLUGIN_API.md)
- [DEVELOPMENT.md](DEVELOPMENT.md)
- [THIRD_PARTY_LICENSES.md](THIRD_PARTY_LICENSES.md)


## Durable local state and workspaces

When the Tauri shell is running, Aegis initializes `aegis.sqlite3` inside the
current user's application-data directory and applies numbered migrations before
registering commands. Conversations, assistant reasoning traces and named
workspace scopes survive restarts. A Vite preview without the native bridge keeps
the best-effort localStorage fallback so the interface remains inspectable.

The **Workspaces** view performs a read-only native metadata check for an existing
directory, canonicalizes the path where the operating system allows it, and
stores the result as a named scope. Registration alone does not expose files to
the model or an agent. That access will be added only with a previewable,
Permission-Broker-mediated tool worker.

The **Model library** view has two real workflows. Provider diagnostics make an
actual request, report route class (local/remote), round-trip latency, retryability
and the provider's live `/models` catalog. Local model registration canonicalizes a
user-selected directory, reads only bounded GGUF headers/metadata, retains valid
models when another file is corrupt, and stores the indexed snapshot in SQLite.
Profiles are saved separately and their weight/KV-cache/RAM/VRAM figures are
explicitly advisory before load. Once the native runtime reports **Ready**, that is
evidence that the process accepted the GGUF; detailed device telemetry remains
unavailable unless the selected runtime exposes it.
