# Windows package verification

The `Windows package` workflow is the release gate for Windows 10/11 x64.
It installs locked frontend dependencies, runs TypeScript and Rust checks,
builds the Tauri application with the MSVC target, and rejects the run unless
at least one installer plus the unpackaged `aegis-ai.exe` are present.

Successful runs publish a `Aegis-AI-Windows-x64` artifact containing:

- `aegis-ai.exe`
- the generated NSIS `.exe` installer
- the generated MSI installer
- `SHA256SUMS.json`

Unsigned development artifacts are for local evaluation. Public releases must
add Windows code signing before the workflow is promoted to a release channel.
