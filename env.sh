#!/usr/bin/env bash
# codetracer-python-recorder Windows dev environment (Git Bash / MSYS2)
# Usage: source env.sh

# Source-safe shell option handling: save caller's options and restore on RETURN.
# Do NOT use `set -euo pipefail` here -- this file is meant to be sourced into
# interactive shells where `-e` would cause the shell to exit on any failing
# command and `-u` would break on unset variables common in interactive sessions.
_ct_pyenv_was_sourced=0
if [[ ${BASH_SOURCE[0]} != "$0" ]]; then
    _ct_pyenv_was_sourced=1
    _ct_pyenv_prev_shellopts=$(set +o)
    trap 'eval "$_ct_pyenv_prev_shellopts"; unset _ct_pyenv_prev_shellopts _ct_pyenv_was_sourced; trap - RETURN' RETURN
fi

set -uo pipefail
if [[ ${_ct_pyenv_was_sourced:-0} -eq 0 ]]; then
    set -e
fi

_ct_pyenv_error() {
    echo "ERROR: $1" >&2
    if [[ ${_ct_pyenv_was_sourced:-0} -eq 1 ]]; then
        return 1
    fi
    exit 1
}

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/non-nix-build/windows" && pwd)"

# Parse toolchain versions
declare -A TOOLCHAIN
while IFS='=' read -r key value; do
    key=$(echo "$key" | tr -d '[:space:]')
    value=$(echo "$value" | tr -d '[:space:]')
    [[ -z "$key" || "$key" == \#* ]] && continue
    TOOLCHAIN[$key]="$value"
done < "$SCRIPT_DIR/toolchain-versions.env"

# Resolve install root with safe cygpath handling
if [[ -z ${WINDOWS_DIY_INSTALL_ROOT:-} ]]; then
    if [[ -n ${LOCALAPPDATA:-} ]]; then
        if command -v cygpath >/dev/null 2>&1; then
            _ct_pyenv_local_app_data=$(cygpath -u "$LOCALAPPDATA")
        else
            _ct_pyenv_local_app_data="$LOCALAPPDATA"
        fi
    else
        _ct_pyenv_local_app_data="$HOME/AppData/Local"
    fi
    INSTALL_ROOT="$_ct_pyenv_local_app_data/codetracer/windows-diy"
    unset _ct_pyenv_local_app_data
else
    INSTALL_ROOT="$WINDOWS_DIY_INSTALL_ROOT"
fi

# Install missing tools via env.ps1 (on-demand)
CARGO_EXE="$INSTALL_ROOT/cargo/bin/cargo.exe"
if [[ ! -f "$CARGO_EXE" ]] || [[ ! -f "$INSTALL_ROOT/capnp/${TOOLCHAIN[CAPNP_VERSION]}/prebuilt/capnproto-tools-win32-${TOOLCHAIN[CAPNP_VERSION]}/capnp.exe" ]] || [[ ! -f "$INSTALL_ROOT/uv/${TOOLCHAIN[UV_VERSION]}/uv.exe" ]]; then
    echo "Installing missing tools via env.ps1..." >&2
    _ct_pyenv_to_windows_path() {
        if command -v cygpath >/dev/null 2>&1; then
            cygpath -w "$1"
        else
            echo "$1"
        fi
    }
    _ct_pyenv_resolve_pwsh() {
        if command -v pwsh >/dev/null 2>&1; then echo "pwsh"; return; fi
        if command -v powershell.exe >/dev/null 2>&1; then echo "powershell.exe"; return; fi
        echo "PowerShell not found" >&2; return 1
    }
    _ct_pyenv_pwsh=$(_ct_pyenv_resolve_pwsh) || _ct_pyenv_error "PowerShell not found"
    _ct_pyenv_root_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
    "$_ct_pyenv_pwsh" -NoProfile -ExecutionPolicy Bypass -File "$(_ct_pyenv_to_windows_path "$_ct_pyenv_root_dir/env.ps1")" || _ct_pyenv_error "Tool installation failed"
fi

export RUSTUP_HOME="$INSTALL_ROOT/rustup"
export CARGO_HOME="$INSTALL_ROOT/cargo"

CAPNP_DIR="$INSTALL_ROOT/capnp/${TOOLCHAIN[CAPNP_VERSION]}/prebuilt/capnproto-tools-win32-${TOOLCHAIN[CAPNP_VERSION]}"
UV_DIR="$INSTALL_ROOT/uv/${TOOLCHAIN[UV_VERSION]}"

# Idempotent PATH update: only prepend entries not already present
_ct_pyenv_path_prepend() {
    local dir="$1"
    case ":$PATH:" in
        *":$dir:"*) ;;
        *) export PATH="$dir:$PATH" ;;
    esac
}
_ct_pyenv_path_prepend "$UV_DIR"
_ct_pyenv_path_prepend "$CAPNP_DIR"
_ct_pyenv_path_prepend "$CARGO_HOME/bin"

echo "rustc: $(rustc --version 2>&1)"
echo "capnp: $(capnp --version 2>&1)"
echo "uv: $(uv --version 2>&1)"
