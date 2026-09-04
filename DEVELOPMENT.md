# Development guide

## Repository rules

- Keep UI, orchestration, permissions, tools and inference contracts separate.
- Do not add a direct execution path from model output to a host-side effect.
- Every new tool starts with a manifest, capability declaration, preview and
  audit strategy.
- Keep large content out of SQLite rows; use attachment/blob references.
- Never commit model files, API keys, local databases or generated installers.
- Never commit the generated `llama-server` binary or its build tree; reproduce it from the pinned source commit.
- Do not treat demo data as production state.
- Add a new numbered migration for schema changes; never edit an applied migration.
- Keep provider secrets out of both SQLite and browser storage; only non-secret UI/provider preferences may use the preview fallback.

## Rust workspace

The Rust workspace contains small crates with one-way dependencies. `protocol`
contains wire primitives only. `permissions` owns the policy boundary. `tools`
and `inference` define adapters, not host-side UI behavior. `core` composes
these pieces without owning backend implementation details.

Run:

```powershell
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

## Frontend

The frontend uses strict TypeScript and Zod validation at the Tauri boundary.
UI state is event-driven; future long transcripts must use virtualization and
buffered stream updates rather than re-rendering the whole Markdown tree per
token.

```powershell
npm install --prefix frontend
npm run typecheck
npm run build
```

### Local provider integration

The current chat transport targets the OpenAI-compatible API surface:
`GET /models` and `POST /chat/completions` under the configured base URL.
Streaming uses server-sent events and accepts both regular content deltas and
the common `reasoning_content`/ `reasoning` delta names.

Provider configuration is deliberately session-oriented. Do not commit API
keys, place them in local storage, or include them in diagnostic logs. Remote
providers should use HTTPS; local development endpoints may use loopback HTTP.
The provider adapter validates message count, message size, sampling values,
base URL credentials, response framing and cancellation before a request can
start.

### Local GGUF catalog checks

The model-library path is a real native workflow, not seeded demo data. Register a
small test directory from the desktop UI, scan it, and verify that valid `.gguf`
files appear with their parsed architecture, context and quantization metadata.
Add a deliberately corrupt `.gguf` file to confirm it is reported as an issue
without hiding the valid model. Re-scan after changing or removing a file and
confirm the SQLite snapshot reflects the current directory.

The scanner must remain metadata-only and bounded: do not add tensor reads, model
execution, symlink traversal or unbounded recursive discovery. A successful
preflight is only an estimate; verify the native runtime panel reaches **Ready**
after `/health` before claiming that a model was loaded. Run the focused tests with:

```powershell
cargo test -p aegis-inference catalog
cargo test -p aegis-database model_library_repository
```

### Native llama.cpp runtime

The desktop's model-file path uses a real `llama-server` process. The build script
fetches the pinned upstream commit, configures a static CPU-native Windows
runtime and writes the binary plus `LLAMA_CPP_BUILD.txt` to the ignored runtime
directory:

```powershell
powershell -ExecutionPolicy Bypass -File .\scripts\build-llama-runtime.ps1
```

This requires Git, CMake and the Visual Studio C++ workload. No model file is
downloaded by the build. For GPU acceleration, build a compatible llama.cpp
server separately and select its executable in the native runtime panel or make
it available on PATH; the app will not silently replace a failed native path with
a remote provider.

After launch, verify the Model library status reaches **Ready** and that the
chat route points to the reported loopback `/v1` endpoint. The runtime panel
should show real prompt/generation counters and throughput after a request;
missing device telemetry must remain unavailable. Stop and reload the model to
exercise process cleanup and generation-safe restart behavior.

## Windows build

Use the MSVC Rust target. The application should run as a standard user. The
installer is configured for current-user installation; features requiring
elevation must be implemented as explicit, narrowly-scoped operations.

Before a release:

1. Build and test on Windows 10 and Windows 11 x64.
2. Exercise the bundled CPU runtime with a small valid GGUF: load, stream, cancel,
   unload and reload it.
3. If an external GPU runtime is supplied, exercise the AMD Vulkan machine,
   including the RX 5700 XT target system, and record the exact runtime build.
4. Validate native process restart after an inference process failure.
5. Inspect the Tauri capability file, packaged `llama-server.exe` hash and generated
   installer permissions.
6. Produce a diagnostic bundle with secrets and personal content redacted.

## Adding a backend

Keep model format parsing, runtime descriptors and UI presentation separate. The
current M2b adapter supervises an external `llama-server` and deliberately reuses
its OpenAI-compatible HTTP surface for stream/reasoning/cancel. A future direct
`InferenceBackend` worker may embed or wrap another native API, but it must expose
capabilities and metrics, reject incompatible profiles with structured errors and
support cancellation without moving C/C++ code into the Tauri process.

## Adding a tool

1. Define a stable manifest and version.
2. Declare the minimum capabilities and risk level.
3. Generate a human-readable preview.
4. Ask the Permission Broker before execution.
5. Consume a one-time permit immediately before the effect.
6. Emit a redacted audit event and wrap external output as untrusted content.



## Durable state and workspace registry

The Tauri shell opens `aegis.sqlite3` in the per-user application-data directory
and applies migrations before commands are exposed. `aegis-database` owns typed
repository records and transaction boundaries. Conversation snapshots preserve
message reasoning and delete through foreign-key cascade. The frontend pauses
native conversation writes while streaming and flushes after a terminal event.

The workspace registry intentionally stops at read-only metadata validation:
`validate_workspace_path` checks existence, directory shape and canonical path
without reading project contents or granting an agent access. When a tool worker is
introduced, it must consume a Permission Broker permit bound to the registered
scope and revalidate it immediately before any effect.
