$ErrorActionPreference = "Stop"

$projectRoot = Split-Path -Parent $PSScriptRoot
$targetCandidates = @(
    (Join-Path $projectRoot "target\x86_64-pc-windows-msvc\release"),
    (Join-Path $projectRoot "apps\desktop\src-tauri\target\x86_64-pc-windows-msvc\release")
)
$targetRoot = $targetCandidates | Where-Object { Test-Path -LiteralPath $_ -PathType Container } | Select-Object -First 1
if (-not $targetRoot) {
    throw "Windows release target directory was not created. Checked: $($targetCandidates -join ', ')"
}
$bundleRoot = Join-Path $targetRoot "bundle"
$artifactRoot = Join-Path $projectRoot "artifacts\windows-x64"

if (-not (Test-Path -LiteralPath $bundleRoot -PathType Container)) {
    throw "Tauri bundle directory was not created: $bundleRoot"
}

$installers = @(
    Get-ChildItem -LiteralPath $bundleRoot -Recurse -File |
        Where-Object { $_.Extension -in @(".exe", ".msi") }
)

if ($installers.Count -eq 0) {
    throw "No NSIS or MSI installer was created under $bundleRoot"
}

if (Test-Path -LiteralPath $artifactRoot) {
    Remove-Item -LiteralPath $artifactRoot -Recurse -Force
}
New-Item -ItemType Directory -Path $artifactRoot | Out-Null

$manifest = foreach ($installer in $installers) {
    $destination = Join-Path $artifactRoot $installer.Name
    Copy-Item -LiteralPath $installer.FullName -Destination $destination
    $hash = Get-FileHash -LiteralPath $destination -Algorithm SHA256
    [PSCustomObject]@{
        File = $installer.Name
        SizeBytes = $installer.Length
        Sha256 = $hash.Hash.ToLowerInvariant()
    }
}

$binary = Join-Path $targetRoot "sendeyizgpt.exe"
if (-not (Test-Path -LiteralPath $binary -PathType Leaf)) {
    throw "The unpackaged desktop executable was not created: $binary"
}
$portableRoot = Join-Path $artifactRoot "SendeyizGPT-Portable"
New-Item -ItemType Directory -Path $portableRoot | Out-Null
Copy-Item -LiteralPath $binary -Destination (Join-Path $portableRoot "SendeyizGPT.exe")
$runtimeSource = Join-Path $projectRoot "runtime\llama.cpp-vulkan"
if (-not (Test-Path -LiteralPath (Join-Path $runtimeSource "llama-server.exe") -PathType Leaf)) {
    throw "Bundled llama.cpp Vulkan runtime was not staged: $runtimeSource"
}
Copy-Item -LiteralPath (Join-Path $projectRoot "runtime") -Destination $portableRoot -Recurse
$portableReadme = @"
SendeyizGPT portable package

1. Run SendeyizGPT.exe.
2. Open Models.
3. Drag a GGUF model file into the window.
4. Choose Balanced, then Load model.
5. Open Chats and start a conversation.

The model and chat database stay on this computer. The bundled inference worker binds only to 127.0.0.1.
"@
Set-Content -LiteralPath (Join-Path $portableRoot "README.txt") -Value $portableReadme -Encoding utf8NoBOM
$portableArchive = Join-Path $artifactRoot "SendeyizGPT-Windows-x64-Portable.zip"
Compress-Archive -Path (Join-Path $portableRoot "*") -DestinationPath $portableArchive -CompressionLevel Optimal
Remove-Item -LiteralPath $portableRoot -Recurse -Force

$manifest += [PSCustomObject]@{
    File = "SendeyizGPT-Windows-x64-Portable.zip"
    SizeBytes = (Get-Item -LiteralPath $portableArchive).Length
    Sha256 = (Get-FileHash -LiteralPath $portableArchive -Algorithm SHA256).Hash.ToLowerInvariant()
}

$manifest |
    Sort-Object File |
    ConvertTo-Json -Depth 3 |
    Set-Content -LiteralPath (Join-Path $artifactRoot "SHA256SUMS.json") -Encoding utf8NoBOM

Write-Host "Verified package contents:" -ForegroundColor Green
$manifest | Sort-Object File | Format-Table -AutoSize
