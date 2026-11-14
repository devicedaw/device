# Packages Device for Windows x64: a portable .zip and an Inno Setup installer.
#
# Usage (from a Visual Studio x64 developer environment, after
#   cargo build --release --locked -p dstudio-app):
#   pwsh tools/release/package-windows.ps1 -Version 0.1.0 -OutDir dist
#
# The app, the plugin scanner (the app looks for it next to itself) and the Visual C++
# runtime DLLs from the build's Visual Studio are staged in one folder, so the app runs on a
# machine without the VC++ redistributable. Needs Inno Setup 6 (ISCC.exe) for the installer.
# Unsigned: SmartScreen warns on first run until code signing is set up.
param(
    [Parameter(Mandatory = $true)][string]$Version,
    [Parameter(Mandatory = $true)][string]$OutDir
)
$ErrorActionPreference = 'Stop'
$root = Resolve-Path (Join-Path $PSScriptRoot '..\..')
$target = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $root 'target' }
$release = Join-Path $target 'release'
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$OutDir = Resolve-Path $OutDir

$stage = Join-Path ([System.IO.Path]::GetTempPath()) "Device-$Version"
if (Test-Path $stage) { Remove-Item -Recurse -Force $stage }
New-Item -ItemType Directory -Path $stage | Out-Null

Copy-Item (Join-Path $release 'device-studio.exe') $stage
# Cargo 1.99 and earlier: build\dstudio-engine-<hash>\out; 1.100 and later: build\dstudio-engine\<hash>\out.
$scanner = Get-ChildItem -Path (Join-Path $release 'build') -Filter 'dstudio_plugin_scanner.exe' -Recurse |
    Where-Object { $_.FullName -match 'dstudio-engine(-[^\\]+|\\[^\\]+)\\out\\build\\' } |
    Sort-Object LastWriteTime -Descending | Select-Object -First 1
if (-not $scanner) { throw 'plugin scanner not found under the release build directory' }
Copy-Item $scanner.FullName $stage

if (-not $env:VCToolsRedistDir) { throw 'VCToolsRedistDir is not set: run from a Visual Studio developer environment' }
$crt = Get-ChildItem -Path (Join-Path $env:VCToolsRedistDir 'x64') -Directory -Filter 'Microsoft.VC*.CRT' | Select-Object -First 1
if (-not $crt) { throw "no Microsoft.VC*.CRT folder under $env:VCToolsRedistDir\x64" }
foreach ($dll in 'msvcp140.dll', 'vcruntime140.dll', 'vcruntime140_1.dll') {
    Copy-Item (Join-Path $crt.FullName $dll) $stage
}
foreach ($doc in 'LICENSE', 'README.md') {
    $p = Join-Path $root $doc
    if (Test-Path $p) { Copy-Item $p $stage }
}
Get-ChildItem $stage | Format-Table Name, Length

$zip = Join-Path $OutDir "Device-$Version-windows-x64.zip"
if (Test-Path $zip) { Remove-Item $zip }
Compress-Archive -Path (Join-Path $stage '*') -DestinationPath $zip

$iscc = Get-Command ISCC.exe -ErrorAction SilentlyContinue
$isccPath = if ($iscc) { $iscc.Source } else { "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe" }
if (-not (Test-Path $isccPath)) { throw 'Inno Setup 6 (ISCC.exe) not found' }
& $isccPath "/DAppVersion=$Version" "/DSourceDir=$stage" "/DOutputDir=$OutDir" (Join-Path $PSScriptRoot 'device-studio.iss')
if ($LASTEXITCODE -ne 0) { throw "ISCC failed ($LASTEXITCODE)" }
Get-ChildItem $OutDir
