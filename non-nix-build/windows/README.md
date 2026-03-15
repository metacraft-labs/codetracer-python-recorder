# codetracer-python-recorder Windows Dev Environment

Standalone Windows dev environment for the Python recorder.

## Quick start

### Bootstrap (first time)
```powershell
pwsh -File non-nix-build\windows\bootstrap-windows-diy.ps1
```

### Activate environment

**Git Bash:**
```sh
source non-nix-build/windows/env.sh
```

**PowerShell:**
```powershell
. .\non-nix-build\windows\env.ps1
```

### Build & test
```sh
uv sync                    # create venv and install deps
uv run maturin develop     # build Rust extension
cargo test                 # Rust tests
uv run pytest              # Python tests
```

## Required tools

| Tool | Version | Purpose |
|------|---------|---------|
| Rust | 1.92.0 | Native extension compilation |
| Cap'n Proto | 1.3.0 | Schema compilation |
| uv | 0.9.28 | Python environment & package management |

## Install location

Shared cache at `%LOCALAPPDATA%\codetracer\windows-diy` (same as main codetracer).
Override with `WINDOWS_DIY_INSTALL_ROOT` environment variable.
