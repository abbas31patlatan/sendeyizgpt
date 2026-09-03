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
