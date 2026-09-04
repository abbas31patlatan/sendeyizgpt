[CmdletBinding()]
param(
    [string]$OutputDirectory,
    [switch]$Force
)

$ErrorActionPreference = "Stop"

$projectRoot = Split-Path -Parent $PSScriptRoot
if ([string]::IsNullOrWhiteSpace($OutputDirectory)) {
    $OutputDirectory = Join-Path $projectRoot "apps\desktop\src-tauri\runtime"
} elseif (-not [System.IO.Path]::IsPathRooted($OutputDirectory)) {
    $OutputDirectory = Join-Path $projectRoot $OutputDirectory
}

$sourceRoot = Join-Path $projectRoot "build\llama.cpp"
$buildRoot = Join-Path $projectRoot "build\llama-build"
$runtimePath = Join-Path $OutputDirectory "llama-server.exe"
$manifestPath = Join-Path $OutputDirectory "LLAMA_CPP_BUILD.txt"
$llamaCppCommit = "427291b5b34cd914a31b3fd3b61a68f6184f4b9f"
$llamaCppRepository = "https://github.com/ggml-org/llama.cpp.git"

function Require-Command([string]$Name, [string]$InstallHint) {
    if (-not (Get-Command $Name -ErrorAction SilentlyContinue)) {
        throw "$Name is required. $InstallHint"
    }
}

function Invoke-Native([string]$Command, [string[]]$Arguments) {
    & $Command @Arguments
    if ($LASTEXITCODE -ne 0) {
        throw "$Command failed with exit code $LASTEXITCODE"
    }
}

Require-Command "git" "Install Git for Windows."
Require-Command "cmake" "Install CMake and the Visual Studio C++ workload."

New-Item -ItemType Directory -Force -Path $OutputDirectory | Out-Null
if (-not $Force -and (Test-Path -LiteralPath $runtimePath -PathType Leaf) -and
    (Test-Path -LiteralPath $manifestPath -PathType Leaf) -and
    ((Get-Content -LiteralPath $manifestPath -Raw) -match [regex]::Escape($llamaCppCommit))) {
    Write-Host "Pinned llama.cpp runtime already exists: $runtimePath" -ForegroundColor Green
    return
}

if (-not (Test-Path -LiteralPath (Join-Path $sourceRoot ".git") -PathType Container)) {
    if (Test-Path -LiteralPath $sourceRoot) {
        throw "The llama.cpp source directory exists but is not a Git checkout: $sourceRoot"
    }
    New-Item -ItemType Directory -Force -Path (Split-Path -Parent $sourceRoot) | Out-Null
    Invoke-Native "git" @("clone", "--filter=blob:none", "--no-checkout", $llamaCppRepository, $sourceRoot)
}
Invoke-Native "git" @("-C", $sourceRoot, "fetch", "--depth", "1", "origin", $llamaCppCommit)
Invoke-Native "git" @("-C", $sourceRoot, "checkout", "--detach", $llamaCppCommit)

$configureArgs = @(
    "-S", $sourceRoot,
    "-B", $buildRoot,
    "-A", "x64",
    "-DBUILD_SHARED_LIBS=OFF",
    "-DGGML_NATIVE=OFF",
    "-DGGML_OPENMP=OFF",
    "-DGGML_VULKAN=OFF",
    "-DGGML_CUDA=OFF",
    "-DGGML_HIP=OFF",
    "-DLLAMA_BUILD_SERVER=ON",
    "-DLLAMA_BUILD_UI=OFF",
    "-DLLAMA_BUILD_TESTS=OFF",
    "-DLLAMA_BUILD_EXAMPLES=OFF",
    "-DLLAMA_BUILD_APP=OFF",
    "-DLLAMA_BUILD_TOOLS=ON",
    "-DLLAMA_OPENSSL=OFF"
)
Invoke-Native "cmake" $configureArgs
Invoke-Native "cmake" @("--build", $buildRoot, "--config", "Release", "--target", "llama-server", "--", "/m")

$builtRuntime = Get-ChildItem -LiteralPath $buildRoot -Recurse -File -Filter "llama-server.exe" |
    Select-Object -First 1
if ($null -eq $builtRuntime) {
    throw "CMake completed but llama-server.exe was not found under $buildRoot"
}
Copy-Item -LiteralPath $builtRuntime.FullName -Destination $runtimePath -Force
$manifest = @(
    "repository=$llamaCppRepository"
    "commit=$llamaCppCommit"
    "configuration=Release; static; CPU portable; Vulkan/CUDA/HIP disabled"
    "source=$sourceRoot"
    "built=$([DateTime]::UtcNow.ToString('o'))"
)
$manifest | Set-Content -LiteralPath $manifestPath -Encoding utf8NoBOM
Write-Host "Built pinned native llama.cpp runtime at $runtimePath" -ForegroundColor Green
