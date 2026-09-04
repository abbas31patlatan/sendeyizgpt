# Aegis AI

Aegis AI is a local-first Windows AI work environment. Its core boundary is
deliberately small: models, agents, tools, providers, events, memory and
workspaces are separate capabilities, while every computer-side effect is
mediated by a native Permission Broker.

## Current status

Milestone 0 foundation and the first M1 chat slice are in place:

- Tauri 2 + React/TypeScript desktop shell
- Rust workspace with typed protocol, IPC authentication helpers, inference and agent contracts
- Permission Broker with workspace path checks, approval records and one-time execution permits
- SQLite migration with foreign keys, WAL and core domain tables
- OpenAI-compatible provider adapter with validation, SSE streaming, cancellation and retry classification
- Responsive bilingual chat/settings UI with Turkish/English copy, system/dark/light themes, searchable conversation history and explicit unavailable surfaces
- Frame-batched streaming updates and debounced browser persistence to keep long responses responsive
- Architecture, security, plugin and development documentation

Native llama.cpp/GGUF inference, model scanning, browser, audio, vision and real
tool execution are not yet wired. Their extension points remain explicit so the
next milestone can add them without changing the chat/provider boundary.

## Prerequisites

Development on Windows requires:

- Windows 10/11 x64
- Visual Studio C++ Build Tools
- Rust stable with the `x86_64-pc-windows-msvc` target
- Node.js 20+ and npm
- Tauri CLI (`cargo install tauri-cli --locked`)
- Vulkan runtime/driver for AMD or other Vulkan-capable GPUs

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
current session and is never persisted by the frontend.

## Interface and performance

The interface follows the operating-system language on first launch and can be
switched between Turkish and English from the top bar. System, dark and light
themes are persisted as non-secret preferences. Recent conversations can be
searched with `Ctrl+K`, created with `Ctrl+N`, copied message-by-message and
deleted with confirmation.

Streaming token and reasoning events are coalesced into animation-frame updates
before React state is changed. Conversation/settings persistence is debounced,
which avoids serializing the full history for every incoming token while
retaining the existing local-first behavior.

## Documents

- [ARCHITECTURE.md](ARCHITECTURE.md)
- [SECURITY.md](SECURITY.md)
- [PLUGIN_API.md](PLUGIN_API.md)
- [DEVELOPMENT.md](DEVELOPMENT.md)
- [THIRD_PARTY_LICENSES.md](THIRD_PARTY_LICENSES.md)
