#!/usr/bin/env bash
# The recorder's Nix package builds from the flake alone, and the store-built
# package actually records.
#
# The crate reaches the trace-format crates by Cargo path
# (`../../codetracer-trace-format/...`), and links the Nim CTFS writer. In a
# Nix sandbox neither sibling checkout exists, so the flake has to supply both.
# When it did not, `nix build` died in maturinBuildHook with
# "failed to read /codetracer-trace-format/codetracer_ctfs/Cargo.toml", while
# every other check stayed green because they all build in the dev shell, where
# the siblings ARE on disk.
#
# A successful build and import would not be enough. `pythonImportsCheck`
# already proves those, and neither reaches the Nim archive's writer: an archive
# from a mismatched revision, or a recorder that loses its runtime dependencies,
# still imports. So this runs what a consumer runs, the package's own
# `bin/codetracer-python-recorder`, and asserts:
#
#   - the recorded program ran under the package's interpreter and the
#     extension came from the store path just built, not from a venv or a
#     checkout on this machine;
#   - `black`, which pyproject.toml declares as a runtime dependency for the
#     autoformat pass, resolves to the store from the package's environment, so
#     the package carries what it declares;
#   - exactly one `.ct` container was written, it starts with the CTFS magic,
#     and `ct-print`, the canonical reader at the revision flake.lock pins,
#     decodes from it the program's own function and its return value.
#
# It runs with a scrubbed environment from a scratch directory, so nothing from
# a dev shell or this checkout can stand in for the package. Nothing is mocked.
# A failed build is a failed test: there is no skip path.
#
# `--no-update-lock-file` makes a flake.lock that misses an input a failure
# rather than a silent, unpinned fetch of the branch tip.
#
# Outputs are rooted by out-links in the scratch directory rather than built
# with `--no-link`. On a host whose auto-GC is running, an unrooted output can
# be deleted between the build and the run (seen on 2026-10-31).
#
#   bash tests/test_nix_package_builds.sh                # both interpreters
#   bash tests/test_nix_package_builds.sh default        # only the named outputs
set -euo pipefail
REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT
fail() { echo "FAIL: $*" >&2; exit 1; }

# The first bytes of every CTFS container: codetracer_ctfs/src/header.rs `MAGIC`
# in codetracer-trace-format, and CTFS_MAGIC in the Python test support.
CTFS_MAGIC_HEX="c0de72ace2"

# The reader, from the input the flake pins, so it matches the writer's revision.
# Stays a fatal failure when it cannot be had: no decoding, no pass.
#
# Built on first use, after a package has built, rather than up front. A flake
# that lacks the input cannot resolve this reference at all, so building it first
# would report that instead of the package's own build failure, which is what
# this test exists to show.
CT_PRINT=""
ensure_ct_print() {
  [ -z "$CT_PRINT" ] || return 0
  local root="$SCRATCH/ct-print.result"
  nix build --out-link "$root" --no-update-lock-file --inputs-from "$REPO" \
    codetracer-trace-format-nim#default \
    || fail "could not build ct-print from the pinned codetracer-trace-format-nim"
  CT_PRINT="$root/bin/ct-print"
}

check_package() {
  local attr="$1" out build_log py_version work trace report stdout decoded
  build_log="$SCRATCH/$attr.build.log"

  if ! out="$(nix build --out-link "$SCRATCH/$attr.result" --no-update-lock-file \
    --print-out-paths "$REPO#$attr" 2>"$build_log")"; then
    tail -n 40 "$build_log" >&2
    fail "nix build .#$attr did not build"
  fi
  [ -n "$out" ] || fail "nix build .#$attr printed no store path"

  # The package names its interpreter, so this needs no version stated here.
  py_version="$(nix eval --raw --no-update-lock-file \
    "$REPO#$attr.pythonModule.pythonVersion")"

  work="$SCRATCH/$attr"
  mkdir -p "$work"
  trace="$work/trace"
  report="$work/report"

  # The program reports on its own process: the recorder runs it in-process, so
  # these are the interpreter and modules the recorder itself is using.
  cat >"$work/program.py" <<'PY'
import importlib.util
import os
import sys

import codetracer_python_recorder

# Located, deliberately not imported. This program is traced, and importing black
# records all of its import: an 11 MB trace whose `ct-print --full` decode took
# about 20 minutes and over 30 GB of memory (measured 2026-10-31). The package's
# runtime-dependency check already refuses a build where black is absent.
black = importlib.util.find_spec("black")


# The name and the result are unusual on purpose: the trace also covers the
# imports above, so a common name or number could match without this function
# having been recorded. The result is computed, so it is not in the source text
# that the trace also embeds; the printed line does reach the trace as text, which
# is why the assertion below looks for a recorded integer.
def nix_package_probe(left, right):
    return left * right


# The compiled extension, rather than the package's pure-Python wrapper: this
# is the file that carries the Rust crate and the Nim writer.
native = [
    module.__file__
    for name, module in sys.modules.items()
    if name.startswith("codetracer_python_recorder")
    and getattr(module, "__file__", "").endswith(".so")
]

with open(os.environ["REPORT"], "w") as report:
    report.write(f"python={sys.version_info.major}.{sys.version_info.minor}\n")
    report.write(f"package={codetracer_python_recorder.__file__}\n")
    report.write(f"black={black.origin if black else 'missing'}\n")
    for path in native:
        report.write(f"native={path}\n")

print("product:", nix_package_probe(1000003, 7))
PY

  # `env -i`: the wrapper must find everything through its own store paths. The
  # PATH is deliberately useless, so a bare `python3` lookup fails here instead
  # of quietly finding some other interpreter.
  if ! stdout="$(cd "$work" && env -i HOME="$work" PATH=/nonexistent REPORT="$report" \
    "$out/bin/codetracer-python-recorder" --out-dir "$trace" program.py 2>&1)"; then
    echo "$stdout" >&2
    fail ".#$attr: the store-built recorder failed to record a program"
  fi

  grep -q '^product: 7000021$' <<<"$stdout" \
    || fail ".#$attr: the recorded program did not run to completion: $stdout"
  [ -f "$report" ] || fail ".#$attr: the recorded program wrote no report"

  grep -qxF "python=$py_version" "$report" \
    || fail ".#$attr: expected the package's Python $py_version: $(cat "$report")"
  grep -q "^package=$out/" "$report" \
    || fail ".#$attr: the recorder did not come from $out: $(cat "$report")"
  grep -q "^native=$out/" "$report" \
    || fail ".#$attr: the native extension did not come from $out: $(cat "$report")"
  grep -q '^black=/nix/store/' "$report" \
    || fail ".#$attr: black was not provided by the package: $(cat "$report")"

  local containers=() magic
  mapfile -t containers < <(find "$trace" -maxdepth 1 -name '*.ct' 2>/dev/null)
  [ "${#containers[@]}" -eq 1 ] \
    || fail ".#$attr: expected one .ct container in $trace, found ${#containers[@]}: $(ls -la "$trace" 2>&1)"
  magic="$(od -An -tx1 -N5 "${containers[0]}" | tr -d ' \n')"
  [ "$magic" = "$CTFS_MAGIC_HEX" ] \
    || fail ".#$attr: ${containers[0]} is not a CTFS container (magic=$magic)"

  # The magic only says the header was written. Decoding proves the Nim writer
  # the package linked produced a trace the reader understands, and that it
  # holds this program's execution.
  #
  # Decoded to a file and searched afterwards, not piped: under `pipefail`, a
  # pipe would report a search that found nothing as a failed decode.
  decoded="$work/decoded.json"
  ensure_ct_print
  if ! "$CT_PRINT" --full "${containers[0]}" >"$decoded" 2>"$work/decode.err"; then
    tail -n 20 "$work/decode.err" >&2
    fail ".#$attr: ct-print could not decode ${containers[0]}"
  fi
  grep -q '"nix_package_probe"' "$decoded" \
    || fail ".#$attr: the trace does not name the recorded function nix_package_probe()"
  # As a recorded integer value, `"i": N`. The bare number is not enough: the
  # program's printed line reaches the trace as text too.
  grep -Eq '"i": *7000021([^0-9]|$)' "$decoded" \
    || fail ".#$attr: the trace does not hold nix_package_probe()'s return value 7000021"

  echo "PASS: .#$attr ($out) recorded under Python $py_version: $(basename "${containers[0]}"), $(wc -c <"${containers[0]}" | tr -d ' ') bytes"
}

# `default` is the declared interpreter. The other is the rest of the support
# matrix, which compiles the extension again against a different ABI.
if [ "$#" -eq 0 ]; then
  set -- default codetracer-python-recorder-python313
fi
for attr in "$@"; do
  check_package "$attr"
done
