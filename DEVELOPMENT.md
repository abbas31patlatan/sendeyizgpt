# Development guide

## Repository rules

- Keep UI, orchestration, permissions, tools and inference contracts separate.
- Do not add a direct execution path from model output to a host-side effect.
- Every new tool starts with a manifest, capability declaration, preview and
  audit strategy.
- Keep large content out of SQLite rows; use attachment/blob references.
- Never commit model files, API keys, local databases or generated installers.
- Do not treat demo data as production state.

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

## Windows build

Use the MSVC Rust target. The application should run as a standard user. The
installer is configured for current-user installation; features requiring
elevation must be implemented as explicit, narrowly-scoped operations.

Before a release:

1. Build and test on Windows 10 and Windows 11 x64.
2. Exercise an AMD Vulkan machine, including the RX 5700 XT target system.
3. Validate worker restart after an inference process failure.
4. Inspect the Tauri capability file and generated installer permissions.
5. Produce a diagnostic bundle with secrets and personal content redacted.

## Adding a backend

Implement `InferenceBackend` in a worker adapter. Keep model format parsing,
runtime descriptors and UI presentation separate. The adapter must expose
capabilities and metrics, reject incompatible profiles with structured errors,
and support cancellation.

## Adding a tool

1. Define a stable manifest and version.
2. Declare the minimum capabilities and risk level.
3. Generate a human-readable preview.
4. Ask the Permission Broker before execution.
5. Consume a one-time permit immediately before the effect.
6. Emit a redacted audit event and wrap external output as untrusted content.

