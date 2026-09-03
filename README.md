# SendeyizGPT

SendeyizGPT is a local-first Windows AI work environment. Its core boundary is
deliberately small: models, agents, tools, providers, events, memory and
workspaces are separate capabilities, while every computer-side effect is
mediated by a native Permission Broker.

## Current status

The current Windows build provides a usable local-chat foundation:

- Tauri 2 + React/TypeScript desktop shell
- isolated llama.cpp server worker with a pinned Windows Vulkan x64 runtime
- GGUF model loading with Eco, Balanced and Performance profiles
- token-streaming local chat with SQLite conversation persistence
- drag-and-drop model selection and an emergency stop control
- Rust workspace with typed protocol, IPC authentication helpers, inference and agent contracts
- Permission Broker with workspace path checks, approval records and one-time execution permits
- SQLite migration with foreign keys, WAL and core domain tables
- Tool manifest and untrusted-output contracts
- Architecture, security, plugin and development documentation

Workspace tools, browser automation, audio, vision, scheduled providers and
plugins remain disabled until their permission/audit implementations are
complete. They are not represented as working features in the UI.

## Start on Windows

1. Extract `SendeyizGPT-Windows-x64-Portable.zip`.
2. Run `SendeyizGPT.exe` (administrator privileges are not required).
3. Open **Models** and drag a `.gguf` model onto the window.
4. Choose **Balanced** and select **Load model**.
5. Open **Chats** and send a message.

The included runtime targets Vulkan and is suitable for AMD Radeon GPUs such
as the RX 5700 XT. A model is not redistributed; the user selects a locally
stored GGUF file whose license is independent from this application.

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

## Documents

- [ARCHITECTURE.md](ARCHITECTURE.md)
- [SECURITY.md](SECURITY.md)
- [PLUGIN_API.md](PLUGIN_API.md)
- [DEVELOPMENT.md](DEVELOPMENT.md)
- [THIRD_PARTY_LICENSES.md](THIRD_PARTY_LICENSES.md)
