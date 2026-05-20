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

# --- Ensure Nim ---
# The recorder's Rust crate depends on codetracer_trace_writer_nim, whose
# build script compiles a Nim static library, so Nim must be on PATH.
# The prebuilt Windows distribution bundles `vccexe`, which lets
# `nim --cc:vcc` locate the MSVC toolchain itself.
$nimVersion = $toolchain["NIM_VERSION"]
$nimDir = Join-Path $installRoot "nim/$nimVersion/nim-$nimVersion"
$nimExe = Join-Path $nimDir "bin/nim.exe"

if (Test-Path $nimExe) {
    Write-Host "Nim $nimVersion already installed"
} else {
    if ($arch -ne "x64") { throw "Nim provisioning in this script only supports x64." }
    Write-Host "Installing Nim $nimVersion..."
    $nimUrl = "https://nim-lang.org/download/nim-${nimVersion}_x64.zip"
    $nimZip = Join-Path $env:TEMP "nim-$nimVersion.zip"
    Invoke-WebRequest -Uri $nimUrl -OutFile $nimZip
    $nimParent = Split-Path -Parent $nimDir
    New-Item -ItemType Directory -Force -Path $nimParent | Out-Null
    Expand-Archive -Path $nimZip -DestinationPath $nimParent -Force
    Remove-Item $nimZip
    Write-Host "Installed Nim to $nimDir"
}

# --- Ensure just ---
# `just` runs the Justfile recipes (the documented dev interface).
$justVersion = $toolchain["JUST_VERSION"]
$justDir = Join-Path $installRoot "just/$justVersion"
$justExe = Join-Path $justDir "just.exe"

if (Test-Path $justExe) {
    Write-Host "just $justVersion already installed"
} else {
    Write-Host "Installing just $justVersion..."
    $justUrl = "https://github.com/casey/just/releases/download/$justVersion/just-$justVersion-x86_64-pc-windows-msvc.zip"
    $justZip = Join-Path $env:TEMP "just-$justVersion.zip"
    Invoke-WebRequest -Uri $justUrl -OutFile $justZip
    New-Item -ItemType Directory -Force -Path $justDir | Out-Null
    Expand-Archive -Path $justZip -DestinationPath $justDir -Force
    Remove-Item $justZip
    Write-Host "Installed just to $justDir"
}

# Set PATH
$pathEntries = @("$cargoHome\bin", $capnpDir, $uvDir, (Join-Path $nimDir "bin"), $justDir)
foreach ($entry in $pathEntries) {
    if ($env:Path -notlike "*$entry*") {
        $env:Path = "$entry;$($env:Path)"
    }
}

# --- Ensure cargo-nextest ---
# `just cargo-test` runs `cargo nextest`. Install the prebuilt binary into
# cargo's bin dir so `cargo nextest` resolves it.
$nextestVersion = $toolchain["CARGO_NEXTEST_VERSION"]
$nextestExe = Join-Path $cargoHome "bin/cargo-nextest.exe"
if (Test-Path $nextestExe) {
    Write-Host "cargo-nextest already installed"
} else {
    Write-Host "Installing cargo-nextest $nextestVersion..."
    $nextestZip = Join-Path $env:TEMP "cargo-nextest-$nextestVersion.zip"
    Invoke-WebRequest -Uri "https://get.nexte.st/$nextestVersion/windows" -OutFile $nextestZip
    Expand-Archive -Path $nextestZip -DestinationPath (Join-Path $cargoHome "bin") -Force
    Remove-Item $nextestZip
    Write-Host "Installed cargo-nextest to $cargoHome\bin"
}

# --- Ensure maturin (PyO3 build backend / CLI) ---
# maturin is provided by the Nix dev shell; on the DIY Windows env install
# it as a uv-managed tool into the shared install root. `just dev` runs
# `uv run ... maturin develop`, which resolves maturin from PATH.
$uvToolDir = Join-Path $installRoot "uv-tools"
$maturinBinDir = Join-Path $uvToolDir "bin"
$env:UV_TOOL_DIR = Join-Path $uvToolDir "tools"
$env:UV_TOOL_BIN_DIR = $maturinBinDir
if (-not (Test-Path (Join-Path $maturinBinDir "maturin.exe"))) {
    Write-Host "Installing maturin via uv..."
    & uv tool install --quiet "maturin>=1.5,<2"
    if ($LASTEXITCODE -ne 0) { throw "uv tool install maturin failed" }
}
if ($env:Path -notlike "*$maturinBinDir*") {
    $env:Path = "$maturinBinDir;$($env:Path)"
}

# --- Python DLL directory on PATH (for PyO3 test binaries) ---
# `cargo test` / `cargo nextest` test binaries for the PyO3 crate link
# libpython and need python3XX.dll at runtime. uv's venv does NOT bundle
# the DLL (it lives next to the base interpreter), so the test exes fail
# to load with 0xc0000135 (DLL not found). Put the base interpreter dir
# on PATH. The base is recorded in .venv/pyvenv.cfg once the venv exists;
# before that, fall back to a uv-managed CPython installation.
$repoRoot = Split-Path -Parent (Split-Path -Parent $scriptDir)
$pyHome = $null
$pyvenvCfg = Join-Path $repoRoot ".venv\pyvenv.cfg"
if (Test-Path $pyvenvCfg) {
    $homeLine = Get-Content $pyvenvCfg | Where-Object { $_ -match '^\s*home\s*=' } | Select-Object -First 1
    if ($homeLine) { $pyHome = ($homeLine -replace '^\s*home\s*=\s*', '').Trim() }
}
if (-not $pyHome) {
    foreach ($line in (& uv python list --only-installed 2>$null)) {
        if ($line -match '(\S+\\python\.exe)\s*$') {
            $cand = $matches[1]
            if ((Test-Path $cand) -and ($cand -notmatch '\.venv')) {
                $pyHome = Split-Path -Parent $cand
                break
            }
        }
    }
}
if ($pyHome -and (Test-Path $pyHome)) {
    if ($env:Path -notlike "*$pyHome*") {
        $env:Path = "$pyHome;$($env:Path)"
    }
    Write-Host "python libpython dir: $pyHome"
} else {
    Write-Host "WARNING: could not resolve the base Python dir; PyO3 cargo tests may fail to find python3XX.dll"
}

Write-Host "rustc: $((& rustc --version) 2>&1)"
Write-Host "capnp: $((& capnp --version) 2>&1)"
Write-Host "uv: $((& uv --version) 2>&1)"
Write-Host "nim: $(((& nim --version 2>&1) | Select-Object -First 1))"
Write-Host "just: $((& just --version) 2>&1)"
