#!/usr/bin/env bash
# Verify that the codetracer-python-recorder CLI complies with
# `codetracer-specs/Recorder-CLI-Conventions.md` (no silent skip — every
# assertion either passes or fails loudly):
#
#   * `--format` is absent from `--help` (CTFS-only — convention §4)
#   * `CODETRACER_FORMAT` is absent from `--help` (convention §5)
#   * `--out-dir` is present in `--help` (§3)
#   * `--version` is present in `--help` (§3)
#   * `--help` mentions `ct print` (the canonical conversion tool, §4)
#   * `CODETRACER_PYTHON_RECORDER_OUT_DIR` is referenced in source so the
#     env-var fallback (§5) cannot regress silently.
#   * `CODETRACER_PYTHON_RECORDER_DISABLED` is referenced in source so the
#     disable-recording env-var (§5) cannot regress silently.
#   * `auto_start.py` no longer reads `CODETRACER_FORMAT` (§5).
#
# Wire-up: see `Justfile` (`just lint` and `just test` both run this
# script) and `pyproject.toml` (the `test:verify-cli-convention` script).
#
# Exit codes:
#   0  all assertions held
#   1  at least one assertion failed (the failing line is printed to
#      stderr and the script exits at the first failure for clarity)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
INNER_REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
OUTER_REPO_ROOT="$(cd "${INNER_REPO_ROOT}/.." && pwd)"

# We invoke the CLI via `python -m codetracer_python_recorder` so the
# verification works whether the wheel is installed system-wide or the
# editable maturin develop build is on PYTHONPATH.  Callers that want
# to override the interpreter (e.g. Nix builds with a wrapped
# interpreter) can set `PYTHON_RECORDER_PYTHON`.
PYTHON_BIN="${PYTHON_RECORDER_PYTHON:-}"
if [[ -z "${PYTHON_BIN}" ]]; then
  if [[ -x "${OUTER_REPO_ROOT}/.venv/bin/python" ]]; then
    PYTHON_BIN="${OUTER_REPO_ROOT}/.venv/bin/python"
  else
    PYTHON_BIN="$(command -v python3 || command -v python || true)"
  fi
fi
if [[ -z "${PYTHON_BIN}" ]] || [[ ! -x "${PYTHON_BIN}" ]]; then
  echo "ERROR: python interpreter not found (set PYTHON_RECORDER_PYTHON)" >&2
  exit 1
fi

# Ensure the editable package is importable.  When run from the venv
# created by `just venv` / `uv sync`, the package is already on
# sys.path; otherwise we fall back to the in-tree wrapper directory.
export PYTHONPATH="${INNER_REPO_ROOT}${PYTHONPATH:+:${PYTHONPATH}}"

run_cli() {
  "${PYTHON_BIN}" -m codetracer_python_recorder "$@"
}

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

assert_absent() {
  # assert_absent <needle> <haystack-description> <haystack>
  local needle="$1"
  local desc="$2"
  local haystack="$3"
  if grep -qF -- "${needle}" <<< "${haystack}"; then
    echo "FAIL: ${desc} must NOT contain '${needle}'" >&2
    echo "----- ${desc} -----" >&2
    echo "${haystack}" >&2
    echo "-------------------" >&2
    exit 1
  fi
  echo "ok: '${needle}' absent from ${desc}"
}

assert_present() {
  # assert_present <needle> <haystack-description> <haystack>
  local needle="$1"
  local desc="$2"
  local haystack="$3"
  if ! grep -qF -- "${needle}" <<< "${haystack}"; then
    echo "FAIL: ${desc} must contain '${needle}'" >&2
    echo "----- ${desc} -----" >&2
    echo "${haystack}" >&2
    echo "-------------------" >&2
    exit 1
  fi
  echo "ok: '${needle}' present in ${desc}"
}

# ---------------------------------------------------------------------------
# `--help` surface
# ---------------------------------------------------------------------------

# argparse prints `--help` to stdout and exits 0; capture it.
TOP_HELP="$(run_cli --help)"

assert_absent "--format" "--help" "${TOP_HELP}"
assert_absent "CODETRACER_FORMAT" "--help" "${TOP_HELP}"
assert_present "--help" "--help" "${TOP_HELP}"
assert_present "--out-dir" "--help" "${TOP_HELP}"
assert_present "--version" "--help" "${TOP_HELP}"
assert_present "ct print" "--help" "${TOP_HELP}"
assert_present "CODETRACER_PYTHON_RECORDER_OUT_DIR" "--help" "${TOP_HELP}"
assert_present "CODETRACER_PYTHON_RECORDER_DISABLED" "--help" "${TOP_HELP}"

# ---------------------------------------------------------------------------
# `--version` surface (must follow `<binary-name> <version>` per §7)
# ---------------------------------------------------------------------------

VERSION_OUT="$(run_cli --version)"
assert_present "codetracer-python-recorder" "--version" "${VERSION_OUT}"

# ---------------------------------------------------------------------------
# Source-level references for env-var fallbacks
# ---------------------------------------------------------------------------

# The recorder must reference `CODETRACER_PYTHON_RECORDER_OUT_DIR` in source
# (otherwise the env-var fallback either doesn't exist or has been
# silently removed).  We grep across the Python wrapper and Rust crate.
if ! grep -rqF "CODETRACER_PYTHON_RECORDER_OUT_DIR" \
       "${INNER_REPO_ROOT}/codetracer_python_recorder" \
       "${INNER_REPO_ROOT}/src"; then
  echo "FAIL: CODETRACER_PYTHON_RECORDER_OUT_DIR must be referenced in codetracer_python_recorder/ or src/" >&2
  exit 1
fi
echo "ok: CODETRACER_PYTHON_RECORDER_OUT_DIR referenced in source"

if ! grep -rqF "CODETRACER_PYTHON_RECORDER_DISABLED" \
       "${INNER_REPO_ROOT}/codetracer_python_recorder" \
       "${INNER_REPO_ROOT}/src"; then
  echo "FAIL: CODETRACER_PYTHON_RECORDER_DISABLED must be referenced in codetracer_python_recorder/ or src/" >&2
  exit 1
fi
echo "ok: CODETRACER_PYTHON_RECORDER_DISABLED referenced in source"

# ---------------------------------------------------------------------------
# auto_start.py no longer reads CODETRACER_FORMAT
# ---------------------------------------------------------------------------

# Pre-2026-05 `auto_start.py` defined `ENV_TRACE_FORMAT = "CODETRACER_FORMAT"`
# and read it at import time.  Convention §5 forbids that; the CTFS-only
# contract makes the env-var meaningless.  Docstring mentions explaining
# the absence of the env-var (i.e. comments / strings inside triple-quoted
# blocks) are tolerated; what we must catch is any *executable* reference,
# i.e. an `os.getenv("CODETRACER_FORMAT", ...)` call or a module-level
# constant binding the name.
if grep -nE '(os\.(getenv|environ).*CODETRACER_FORMAT|^[A-Z_]+\s*=\s*"CODETRACER_FORMAT")' \
   "${INNER_REPO_ROOT}/codetracer_python_recorder/auto_start.py"; then
  echo "FAIL: auto_start.py must not read or bind CODETRACER_FORMAT (CTFS-only)" >&2
  exit 1
fi
echo "ok: auto_start.py has no executable CODETRACER_FORMAT reference"

# ---------------------------------------------------------------------------
# cli.py no longer wires `--format`
# ---------------------------------------------------------------------------

# The CLI source must not declare `--format`.  We tolerate the string
# inside error-message literals (where we tell users the flag is gone)
# but the argparse `add_argument("--format", ...)` registration must be
# absent.
if grep -nE 'add_argument\(\s*"--format"' \
   "${INNER_REPO_ROOT}/codetracer_python_recorder/cli.py"; then
  echo "FAIL: cli.py must not declare --format (CTFS-only)" >&2
  exit 1
fi
echo "ok: cli.py has no --format argparse registration"

echo "verify-cli-convention-no-silent-skip: all checks passed"
