# Windows package verification

The `Windows package` workflow is the release gate for Windows 10/11 x64.
It installs locked frontend dependencies, runs TypeScript and Rust checks,
downloads and checksum-verifies the pinned official llama.cpp Vulkan runtime,
builds the Tauri application with the MSVC target, and rejects the run unless
the installers, desktop binary and isolated inference worker are present.

Successful runs publish an `Aegis-AI-Windows-x64` CI artifact containing:

- `SendeyizGPT-Windows-x64-Portable.zip` with `SendeyizGPT.exe` and the Vulkan runtime
- the generated NSIS `.exe` installer
- the generated MSI installer
- `SHA256SUMS.json`

Unsigned development artifacts are for local evaluation. Public releases must
add Windows code signing before the workflow is promoted to a release channel.
