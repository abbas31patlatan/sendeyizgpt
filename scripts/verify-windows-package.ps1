$ErrorActionPreference = "Stop"

$projectRoot = Split-Path -Parent $PSScriptRoot
$targetRoot = Join-Path $projectRoot "apps\desktop\src-tauri\target\x86_64-pc-windows-msvc\release"
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

$binary = Join-Path $targetRoot "aegis-ai.exe"
if (-not (Test-Path -LiteralPath $binary -PathType Leaf)) {
    throw "The unpackaged desktop executable was not created: $binary"
}
Copy-Item -LiteralPath $binary -Destination (Join-Path $artifactRoot "aegis-ai.exe")

$binaryInfo = Get-Item -LiteralPath $binary
$binaryHash = Get-FileHash -LiteralPath $binary -Algorithm SHA256
$manifest += [PSCustomObject]@{
    File = "aegis-ai.exe"
    SizeBytes = $binaryInfo.Length
    Sha256 = $binaryHash.Hash.ToLowerInvariant()
}

$manifest |
    Sort-Object File |
    ConvertTo-Json -Depth 3 |
    Set-Content -LiteralPath (Join-Path $artifactRoot "SHA256SUMS.json") -Encoding utf8NoBOM

Write-Host "Verified package contents:" -ForegroundColor Green
$manifest | Sort-Object File | Format-Table -AutoSize

