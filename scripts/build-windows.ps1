$ErrorActionPreference = "Stop"

$projectRoot = Split-Path -Parent $PSScriptRoot
Set-Location $projectRoot

function Require-Command([string]$Name, [string]$InstallHint) {
    if (-not (Get-Command $Name -ErrorAction SilentlyContinue)) {
        throw "$Name is required. $InstallHint"
    }
}

Require-Command "cargo" "Install Rust with the stable MSVC toolchain."
Require-Command "rustup" "Install rustup from https://rustup.rs."
Require-Command "npm" "Install the Node.js LTS release."

$llamaVersion = "b10785"
$llamaSha256 = "08cf48c8ccdb56eaa9e2aed4f08abe9ad9994edf81961aed265d195aecd835e5"
$llamaAsset = "llama-$llamaVersion-bin-win-vulkan-x64.zip"
$llamaArchive = Join-Path $env:TEMP $llamaAsset
$llamaRuntime = Join-Path $projectRoot "runtime\llama.cpp-vulkan"
Write-Host "Fetching the pinned llama.cpp Vulkan runtime..." -ForegroundColor Cyan
Invoke-WebRequest -Uri "https://github.com/ggml-org/llama.cpp/releases/download/$llamaVersion/$llamaAsset" -OutFile $llamaArchive
$actualLlamaHash = (Get-FileHash -LiteralPath $llamaArchive -Algorithm SHA256).Hash.ToLowerInvariant()
if ($actualLlamaHash -ne $llamaSha256) {
    throw "llama.cpp runtime checksum mismatch. Expected $llamaSha256, got $actualLlamaHash"
}
if (Test-Path -LiteralPath $llamaRuntime) {
    Remove-Item -LiteralPath $llamaRuntime -Recurse -Force
}
New-Item -ItemType Directory -Path $llamaRuntime | Out-Null
Expand-Archive -LiteralPath $llamaArchive -DestinationPath $llamaRuntime
if (-not (Test-Path -LiteralPath (Join-Path $llamaRuntime "llama-server.exe") -PathType Leaf)) {
    throw "The pinned runtime archive does not contain llama-server.exe"
}

Write-Host "Checking the Windows MSVC Rust target..." -ForegroundColor Cyan
rustup default stable-msvc
rustup target add x86_64-pc-windows-msvc

Write-Host "Installing locked frontend dependencies..." -ForegroundColor Cyan
npm ci --prefix frontend
npm ci --prefix apps/desktop

Write-Host "Running frontend checks..." -ForegroundColor Cyan
npm --prefix frontend run typecheck
npm --prefix frontend run build

Write-Host "Running Rust formatting, lint and tests..." -ForegroundColor Cyan
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace

Write-Host "Building the Tauri release bundles..." -ForegroundColor Cyan
npm --prefix apps/desktop exec -- tauri build --target x86_64-pc-windows-msvc

Write-Host "Verifying and staging Windows artifacts..." -ForegroundColor Cyan
& (Join-Path $PSScriptRoot "verify-windows-package.ps1")

$artifactPath = Join-Path $projectRoot "artifacts/windows-x64"
Write-Host "Build completed. Verified files are under: $artifactPath" -ForegroundColor Green
