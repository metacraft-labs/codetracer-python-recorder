# codetracer-python-recorder Windows dev environment (PowerShell)
# Usage: . .\env.ps1

$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"
$scriptDir = Join-Path (Split-Path -Parent $MyInvocation.MyCommand.Definition) "non-nix-build\windows"

# Parse toolchain versions
$toolchainFile = Join-Path $scriptDir "toolchain-versions.env"
$toolchain = @{}
Get-Content $toolchainFile | ForEach-Object {
    $line = $_.Trim()
    if ($line -and -not $line.StartsWith("#")) {
        $parts = $line -split "=", 2
        $toolchain[$parts[0].Trim()] = $parts[1].Trim()
    }
}

$installRoot = if ($env:WINDOWS_DIY_INSTALL_ROOT) { $env:WINDOWS_DIY_INSTALL_ROOT }
               else { Join-Path $env:LOCALAPPDATA "codetracer/windows-diy" }
New-Item -ItemType Directory -Force -Path $installRoot | Out-Null

$arch = if ((Get-CimInstance Win32_ComputerSystem).SystemType -match "ARM") { "arm64" } else { "x64" }

# --- Ensure Rust ---
$rustupHome = Join-Path $installRoot "rustup"
$cargoHome = Join-Path $installRoot "cargo"
$env:RUSTUP_HOME = $rustupHome
$env:CARGO_HOME = $cargoHome
$rustcExe = Join-Path $cargoHome "bin/rustc.exe"
$rustupExe = Join-Path $cargoHome "bin/rustup.exe"
$rustToolchain = $toolchain["RUST_TOOLCHAIN_VERSION"]

$needRust = $true
if (Test-Path $rustcExe) {
    $rustcVer = (& $rustcExe --version 2>&1)
    if ($rustcVer -match "^rustc $([regex]::Escape($rustToolchain)) ") {
        Write-Host "Rust $rustToolchain already installed"
        $needRust = $false
    }
}
if ($needRust) {
    Write-Host "Installing Rust $rustToolchain..."
    New-Item -ItemType Directory -Force -Path $rustupHome | Out-Null
    New-Item -ItemType Directory -Force -Path $cargoHome | Out-Null
    $rustupInit = Join-Path $env:TEMP "rustup-init.exe"
    $target = if ($arch -eq "arm64") { "aarch64-pc-windows-msvc" } else { "x86_64-pc-windows-msvc" }
    $rustupUrl = "https://static.rust-lang.org/rustup/dist/$target/rustup-init.exe"
    Invoke-WebRequest -Uri $rustupUrl -OutFile $rustupInit
    & $rustupInit --default-toolchain $rustToolchain --profile minimal -y --no-modify-path
    if ($LASTEXITCODE -ne 0) { throw "rustup-init failed" }
    Remove-Item $rustupInit -Force -ErrorAction SilentlyContinue
}
& $rustupExe component add clippy 2>&1 | Out-Null

# --- Ensure Cap'n Proto ---
$capnpVersion = $toolchain["CAPNP_VERSION"]
$capnpDir = Join-Path $installRoot "capnp/$capnpVersion/prebuilt/capnproto-tools-win32-$capnpVersion"
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
    $expected = $toolchain["CAPNP_WIN_X64_SHA256"]
    if ($hash -ne $expected) { throw "Cap'n Proto SHA256 mismatch: got $hash, expected $expected" }
    $capnpParent = Split-Path -Parent $capnpDir
    New-Item -ItemType Directory -Force -Path $capnpParent | Out-Null
    Expand-Archive -Path $capnpZip -DestinationPath $capnpParent -Force
    Remove-Item $capnpZip
    Write-Host "Installed Cap'n Proto to $capnpDir"
}

# --- Ensure uv ---
$uvVersion = $toolchain["UV_VERSION"]
$uvDir = Join-Path $installRoot "uv/$uvVersion"
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
    $nested = Get-ChildItem -Path $uvDir -Directory | Where-Object { Test-Path (Join-Path $_.FullName "uv.exe") } | Select-Object -First 1
    if ($nested) {
        Get-ChildItem -Path $nested.FullName | Move-Item -Destination $uvDir -Force
        Remove-Item $nested.FullName -Recurse -Force
    }
    Remove-Item $uvZip
    Write-Host "Installed uv to $uvDir"
}

# Set PATH
$pathEntries = @("$cargoHome\bin", $capnpDir, $uvDir)
foreach ($entry in $pathEntries) {
    if ($env:Path -notlike "*$entry*") {
        $env:Path = "$entry;$($env:Path)"
    }
}

Write-Host "rustc: $((& rustc --version) 2>&1)"
Write-Host "capnp: $((& capnp --version) 2>&1)"
Write-Host "uv: $((& uv --version) 2>&1)"
