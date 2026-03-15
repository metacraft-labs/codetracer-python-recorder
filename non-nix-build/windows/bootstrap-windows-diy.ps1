<#
.SYNOPSIS
  Bootstrap Windows DIY dev environment for codetracer-python-recorder.
  Installs: Rust toolchain (via rustup), Cap'n Proto (prebuilt), uv (Python manager).

.DESCRIPTION
  Content-addressable, idempotent bootstrap. Safe to re-run.
  Install root: $env:LOCALAPPDATA/codetracer/windows-diy (shared with codetracer)
#>
param(
    [string]$InstallRoot = $(
        if ($env:WINDOWS_DIY_INSTALL_ROOT) { $env:WINDOWS_DIY_INSTALL_ROOT }
        else { Join-Path $env:LOCALAPPDATA "codetracer/windows-diy" }
    )
)

$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"

$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$toolchainFile = Join-Path $scriptDir "toolchain-versions.env"

# Parse toolchain-versions.env
$toolchain = @{}
Get-Content $toolchainFile | ForEach-Object {
    $line = $_.Trim()
    if ($line -and -not $line.StartsWith("#")) {
        $parts = $line -split "=", 2
        $toolchain[$parts[0].Trim()] = $parts[1].Trim()
    }
}

$resolvedRoot = (New-Item -ItemType Directory -Force -Path $InstallRoot).FullName
Write-Host "Install root: $resolvedRoot"

function Get-WindowsArch {
    $sys = (Get-CimInstance Win32_ComputerSystem).SystemType
    if ($sys -match "ARM") { return "arm64" }
    return "x64"
}
$arch = Get-WindowsArch
Write-Host "Architecture: $arch"

# --- Rust (via rustup) ---
$rustupHome = Join-Path $resolvedRoot "rustup"
$cargoHome = Join-Path $resolvedRoot "cargo"
$cargoExe = Join-Path $cargoHome "bin/cargo.exe"
$env:RUSTUP_HOME = $rustupHome
$env:CARGO_HOME = $cargoHome

if (Test-Path $cargoExe) {
    Write-Host "Rust already installed at $cargoHome"
} else {
    Write-Host "Installing Rust $($toolchain.RUST_TOOLCHAIN_VERSION)..."
    $rustupInit = Join-Path $env:TEMP "rustup-init.exe"
    $rustupUrl = "https://static.rust-lang.org/rustup/dist/x86_64-pc-windows-msvc/rustup-init.exe"
    if ($arch -eq "arm64") { $rustupUrl = "https://static.rust-lang.org/rustup/dist/aarch64-pc-windows-msvc/rustup-init.exe" }
    Invoke-WebRequest -Uri $rustupUrl -OutFile $rustupInit
    & $rustupInit --default-toolchain $toolchain.RUST_TOOLCHAIN_VERSION --profile minimal -y --no-modify-path
    if ($LASTEXITCODE -ne 0) { throw "rustup-init failed" }
}
# Ensure clippy
& (Join-Path $cargoHome "bin/rustup.exe") component add clippy 2>&1 | Out-Null

# --- Cap'n Proto (prebuilt x64) ---
$capnpVersion = $toolchain.CAPNP_VERSION
$capnpDir = Join-Path $resolvedRoot "capnp/$capnpVersion/prebuilt/capnproto-tools-win32-$capnpVersion"
$capnpExe = Join-Path $capnpDir "capnp.exe"

if (Test-Path $capnpExe) {
    Write-Host "Cap'n Proto $capnpVersion already installed"
} else {
    if ($arch -ne "x64") { throw "Cap'n Proto prebuilt only available for x64" }
    Write-Host "Installing Cap'n Proto $capnpVersion..."
    $capnpUrl = "https://capnproto.org/capnproto-c++-win32-$capnpVersion.zip"
    $capnpZip = Join-Path $env:TEMP "capnp-$capnpVersion.zip"
    Invoke-WebRequest -Uri $capnpUrl -OutFile $capnpZip

    $hash = (Get-FileHash -Path $capnpZip -Algorithm SHA256).Hash
    $expected = $toolchain.CAPNP_WIN_X64_SHA256
    if ($hash -ne $expected) { throw "Cap'n Proto SHA256 mismatch: got $hash, expected $expected" }

    $capnpParent = Split-Path -Parent $capnpDir
    New-Item -ItemType Directory -Force -Path $capnpParent | Out-Null
    Expand-Archive -Path $capnpZip -DestinationPath $capnpParent -Force
    Remove-Item $capnpZip
    Write-Host "Installed Cap'n Proto to $capnpDir"
}

# --- uv (Python toolchain manager) ---
$uvVersion = $toolchain.UV_VERSION
$uvDir = Join-Path $resolvedRoot "uv/$uvVersion"
$uvExe = Join-Path $uvDir "uv.exe"

if (Test-Path $uvExe) {
    Write-Host "uv $uvVersion already installed"
} else {
    Write-Host "Installing uv $uvVersion..."
    $uvTarget = if ($arch -eq "arm64") { "aarch64-pc-windows-msvc" } else { "x86_64-pc-windows-msvc" }
    $uvUrl = "https://github.com/astral-sh/uv/releases/download/$uvVersion/uv-$uvTarget.zip"
    $uvZip = Join-Path $env:TEMP "uv-$uvVersion.zip"
    Invoke-WebRequest -Uri $uvUrl -OutFile $uvZip
    New-Item -ItemType Directory -Force -Path $uvDir | Out-Null
    Expand-Archive -Path $uvZip -DestinationPath $uvDir -Force
    # uv extracts into a subdirectory - flatten if needed
    $nested = Get-ChildItem -Path $uvDir -Directory | Where-Object { Test-Path (Join-Path $_.FullName "uv.exe") } | Select-Object -First 1
    if ($nested) {
        Get-ChildItem -Path $nested.FullName | Move-Item -Destination $uvDir -Force
        Remove-Item $nested.FullName -Recurse -Force
    }
    Remove-Item $uvZip
    Write-Host "Installed uv to $uvDir"
}

Write-Host "`nBootstrap complete."
Write-Host "RUSTUP_HOME=$rustupHome"
Write-Host "CARGO_HOME=$cargoHome"
Write-Host "CAPNP_DIR=$capnpDir"
Write-Host "UV_DIR=$uvDir"
