#!/usr/bin/env bash
# Entering this repository's dev shell from ANOTHER git repository writes
# nothing into that repository.
#
# The dev shell's hook installs git hooks (`.pre-commit-config.yaml`, hooks
# under `.git/hooks`, `core.hooksPath`). git-hooks.nix aims them at
# `git rev-parse --show-toplevel` of the CURRENT DIRECTORY, so without a guard
# `nix develop /path/to/this-repo` run from a sibling checkout plants this
# repository's hooks there: an untracked file that blocks that repository's
# pre-push gate, and foreign checks on its commits. flake.nix runs the hook only
# when the enclosing repository is this one.
#
# Asserted, from a scratch git repository and from a subdirectory of it: no
# file or directory appears, no hook is
# installed and `core.hooksPath` is untouched. As the positive control, entered
# from inside this repository the hook still installs `.pre-commit-config.yaml`.
#
#   bash tests/test_dev_shell_writes_nothing_elsewhere.sh
set -euo pipefail
REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT
fail() { echo "FAIL: $*" >&2; exit 1; }

git -C "$SCRATCH" init -q
git -C "$SCRATCH" -c user.name=t -c user.email=t@t commit -q --allow-empty -m init
mkdir -p "$SCRATCH/sub"
hooks_before="$(ls "$SCRATCH/.git/hooks")"

for dir in "$SCRATCH" "$SCRATCH/sub"; do
  ( cd "$dir" && nix develop "$REPO" --no-write-lock-file -c true ) >/dev/null 2>&1 \
    || fail "the dev shell did not start from $dir"
  [ -z "$(git -C "$SCRATCH" status --porcelain --ignored)" ] \
    || fail "entered from $dir: files were written into the other repository: $(git -C "$SCRATCH" status --porcelain --ignored | tr '\n' ' ')"
  # git status does not list empty directories.
  extra="$(cd "$SCRATCH" && find . -mindepth 1 -path ./.git -prune -o ! -path ./sub -print)"
  [ -z "$extra" ] || fail "entered from $dir: entries were created in the other repository: $(echo $extra)"
  [ "$(ls "$SCRATCH/.git/hooks")" = "$hooks_before" ] \
    || fail "entered from $dir: git hooks were installed into the other repository"
  [ -z "$(git -C "$SCRATCH" config --local --get core.hooksPath || true)" ] \
    || fail "entered from $dir: the other repository's core.hooksPath was changed"
done

# Positive control: the guard must not stop the hook in this repository.
if [ -L "$REPO/.pre-commit-config.yaml" ]; then rm -f "$REPO/.pre-commit-config.yaml"; fi
( cd "$REPO" && nix develop "$REPO" --no-write-lock-file -c true ) >/dev/null 2>&1 \
  || fail "the dev shell did not start from this repository"
[ -L "$REPO/.pre-commit-config.yaml" ] \
  || fail "control: entered from this repository, the hook did not install .pre-commit-config.yaml"

echo "PASS: entered from another repository the dev shell writes nothing there; from this one it installs its hooks"
