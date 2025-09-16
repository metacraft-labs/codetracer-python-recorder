#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'USAGE'
Usage:
  agent-workspace.sh run <workspace-id> [--workflow NAME] [--base-change CHANGE] [--cleanup] [--no-direnv] -- COMMAND [ARG...]
  agent-workspace.sh status [<workspace-id>]
  agent-workspace.sh shell <workspace-id>
  agent-workspace.sh clean <workspace-id>
  agent-workspace.sh sync-tools <workspace-id>
USAGE
}

fail() {
  echo "agent-workspace: $*" >&2
  exit 1
}

require() {
  if [[ "$1" != "0" ]]; then
    fail "$2"
  fi
}

repo_root=$(jj root)
repo_root=${repo_root%$'\n'}
repo_basename=$(basename "$repo_root")
repo_hash=$(printf '%s' "$repo_root" | sha256sum | cut -c1-10)
repo_slug="${repo_basename}-${repo_hash}"

cache_root_default=${XDG_CACHE_HOME:-"$HOME/.cache"}
workspace_root=${AI_WORKSPACES_ROOT:-"$cache_root_default/ai-workspaces"}
workspace_repo_root="$workspace_root/$repo_slug"

tools_source_root="${AGENT_TOOLS_SOURCE:-$repo_root}"
tools_relative_paths=("agents.just" "rules" "scripts")

sanitise_workspace_id() {
  local id="$1"
  [[ "$id" =~ ^[A-Za-z0-9._-]+$ ]] || fail "workspace id '$id' contains invalid characters"
  echo "$id"
}

workspace_path_for() {
  local workspace_id="$1"
  echo "$workspace_repo_root/$workspace_id"
}

metadata_path_for() {
  local workspace_id="$1"
  echo "$(workspace_path_for "$workspace_id")/.agent-tools/.agent-workflow.json"
}

workspace_registered() {
  local workspace_id="$1"
  jj workspace list -T 'name ++ "\n"' | grep -Fx "$workspace_id" >/dev/null 2>&1
}

compute_tools_hash() {
  python - "$tools_source_root" "${tools_relative_paths[@]}" <<'PY'
import hashlib
import os
import sys
from pathlib import Path

root = Path(sys.argv[1]).resolve()
paths = sys.argv[2:]

hasher = hashlib.sha256()

def add_file(path: Path):
    rel = path.relative_to(root)
    hasher.update(str(rel).encode('utf-8'))
    hasher.update(b'\0')
    with path.open('rb') as fh:
        while True:
            chunk = fh.read(65536)
            if not chunk:
                break
            hasher.update(chunk)

if not paths:
    print('')
    raise SystemExit(0)

for rel in paths:
    src = root / rel
    if not src.exists():
        print(f"missing:{rel}", file=sys.stderr)
        raise SystemExit(1)
    if src.is_file():
        add_file(src)
    else:
        for file_path in sorted(src.rglob('*')):
            if file_path.is_file():
                add_file(file_path)

print(hasher.hexdigest())
PY
}

copy_tools_payload() {
    local dest="$1"
    echo "*" > "$dest/.gitignore"
  python - "$tools_source_root" "$dest" "${tools_relative_paths[@]}" <<'PY'
import shutil
import sys
from pathlib import Path

root = Path(sys.argv[1]).resolve()
dest = Path(sys.argv[2]).resolve()
paths = sys.argv[3:]

for rel in paths:
    src = root / rel
    if not src.exists():
        raise SystemExit(f"tool path missing: {rel}")
    target = dest / rel
    if src.is_dir():
        shutil.copytree(src, target, dirs_exist_ok=True)
    else:
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(src, target)
PY
}

prepare_tools_copy() {
  local workspace_path="$1"
  local force_copy="${2:-false}"

  local tools_dest="$workspace_path/.agent-tools"
  TOOLS_COPY_PATH="$tools_dest"

  local desired_hash
  desired_hash=$(compute_tools_hash) || fail "failed to hash tool sources"
  TOOLS_VERSION="$desired_hash"

  local version_file="$tools_dest/.version"
  local current_hash=""
  if [[ -f "$version_file" ]]; then
    current_hash=$(cat "$version_file")
  fi

  if [[ "$force_copy" == "true" ]]; then
    current_hash=""
  fi

  if [[ "$current_hash" != "$desired_hash" ]]; then
    case "$tools_dest" in
      "$workspace_path"/*) ;;
      *) fail "tool copy path outside workspace: $tools_dest" ;;
    esac
    rm -rf "$tools_dest"
    mkdir -p "$tools_dest"
    copy_tools_payload "$tools_dest"
    printf '%s\n' "$desired_hash" > "$version_file"
  else
    mkdir -p "$tools_dest"
  fi
}

ensure_workspace() {
  local workspace_id="$1"
  local workspace_path="$2"
  local created_var="$3"

  mkdir -p "$workspace_repo_root"

  if workspace_registered "$workspace_id"; then
    local metadata_path
    metadata_path=$(metadata_path_for "$workspace_id")
    if [[ -f "$metadata_path" ]]; then
      local recorded_path
      recorded_path=$(python - "$metadata_path" <<'PY'
import json
import sys
path = sys.argv[1]
with open(path, 'r', encoding='utf-8') as fh:
    data = json.load(fh)
print(data.get('workspace_path', ''))
PY
      )
      if [[ -n "$recorded_path" && "$recorded_path" != "$workspace_path" ]]; then
        fail "workspace '$workspace_id' already exists at '$recorded_path'"
      fi
    fi
    printf -v "$created_var" '%s' "false"
  else
    jj workspace add --name "$workspace_id" "$workspace_path"
    printf -v "$created_var" '%s' "true"
  fi
}

update_metadata() {
  local metadata_path="$1"
  local status="$2"
  local timestamp
  timestamp=$(date -Iseconds)

  STATUS="$status" TIMESTAMP="$timestamp" WORKSPACE_ID="$WORKSPACE_ID" REPO_ROOT="$repo_root" \
  WORKSPACE_PATH="$WORKSPACE_PATH" WORKFLOW_NAME="${WORKFLOW_NAME:-}" COMMAND_JSON="$COMMAND_JSON" \
  DIRENV_ALLOWED="$DIRENV_ALLOWED" BASE_CHANGE="${BASE_CHANGE:-}" TOOLS_SOURCE="${TOOLS_SOURCE:-}" \
  TOOLS_COPY="${TOOLS_COPY:-}" TOOLS_VERSION="${TOOLS_VERSION:-}" \
  python - "$metadata_path" <<'PY'
import json
import os
import sys

path = sys.argv[1]
now = os.environ["TIMESTAMP"]
status = os.environ["STATUS"]
workspace_id = os.environ["WORKSPACE_ID"]
repo_root = os.environ["REPO_ROOT"]
workspace_path = os.environ["WORKSPACE_PATH"]
workflow_name = os.environ.get("WORKFLOW_NAME") or None
direnv_allowed = os.environ.get("DIRENV_ALLOWED", "false").lower() == "true"
command = json.loads(os.environ.get("COMMAND_JSON", "[]"))
base_change = os.environ.get("BASE_CHANGE") or None
tools_source = os.environ.get("TOOLS_SOURCE") or None
tools_copy = os.environ.get("TOOLS_COPY") or None
tools_version = os.environ.get("TOOLS_VERSION") or None

try:
    with open(path, "r", encoding="utf-8") as fh:
        data = json.load(fh)
except Exception:
    data = {}

if "created_at" not in data:
    data["created_at"] = now

data.update({
    "workspace_id": workspace_id,
    "repo_root": repo_root,
    "workspace_path": workspace_path,
    "status": status,
    "direnv_allowed": direnv_allowed,
    "command": command,
    "updated_at": now,
})

if workflow_name is None:
    data.pop("workflow", None)
else:
    data["workflow"] = workflow_name

if base_change is None:
    data.pop("base_change", None)
else:
    data["base_change"] = base_change

if tools_source is None:
    data.pop("tools_source", None)
else:
    data["tools_source"] = tools_source

if tools_copy is None:
    data.pop("tools_copy", None)
else:
    data["tools_copy"] = tools_copy

if tools_version is None:
    data.pop("tools_version", None)
else:
    data["tools_version"] = tools_version

with open(path, "w", encoding="utf-8") as fh:
    json.dump(data, fh, indent=2)
    fh.write("\n")
PY
}

run_subcommand() {
  local workspace_id_raw="$1"
  shift || fail "missing options and command"

  local workspace_id
  workspace_id=$(sanitise_workspace_id "$workspace_id_raw")
  WORKSPACE_ID="$workspace_id"

  local workflow_name=""
  local base_change=""
  local cleanup="false"
  local use_direnv="true"
  local positional_found="false"
  local cmd=()

  while [[ $# -gt 0 ]]; do
    case "$1" in
      --workflow)
        [[ $# -ge 2 ]] || fail "--workflow requires a value"
        workflow_name="$2"
        shift 2
        ;;
      --base-change)
        [[ $# -ge 2 ]] || fail "--base-change requires a value"
        base_change="$2"
        shift 2
        ;;
      --cleanup)
        cleanup="true"
        shift
        ;;
      --no-direnv)
        use_direnv="false"
        shift
        ;;
      --)
        shift
        positional_found="true"
        cmd=("$@")
        break
        ;;
      *)
        fail "unknown option '$1'"
        ;;
    esac
  done

  [[ "$positional_found" == "true" ]] || fail "missing command after '--'"
  [[ ${#cmd[@]} -gt 0 ]] || fail "command must not be empty"

  WORKFLOW_NAME="$workflow_name"
  BASE_CHANGE="$base_change"

  local workspace_path="$workspace_repo_root/$workspace_id"
  WORKSPACE_PATH="$workspace_path"
  local metadata_path="$workspace_path/.agent-tools/.agent-workflow.json"
  METADATA_PATH="$metadata_path"

  local created_flag
  ensure_workspace "$workspace_id" "$workspace_path" created_flag

  if [[ -n "$base_change" && "$created_flag" == "true" ]]; then
    (cd "$workspace_path" && jj edit "$base_change")
  fi

  prepare_tools_copy "$workspace_path"

  local direnv_status="false"
  if [[ "$use_direnv" == "true" ]]; then
    command -v direnv >/dev/null 2>&1 || fail "direnv is required but not installed"
    (
      cd "$workspace_path"
      direnv allow .
    )
    direnv_status="true"
  fi
  DIRENV_ALLOWED="$direnv_status"
  TOOLS_SOURCE="$tools_source_root"

  COMMAND_JSON=$(python - <<'PY' "${cmd[@]}"
import json
import sys
print(json.dumps(sys.argv[1:]))
PY
)

  TOOLS_COPY="$TOOLS_COPY_PATH"
  update_metadata "$metadata_path" "running"

  local exit_code
  (
    cd "$workspace_path"
    if [[ "$use_direnv" == "true" ]]; then
      AGENT_WORKSPACE_ID="$workspace_id" \
      AGENT_WORKSPACE_PATH="$workspace_path" \
      AGENT_WORKSPACE_METADATA="$metadata_path" \
      AGENT_WORKSPACE_REPO_ROOT="$repo_root" \
      AGENT_TOOL_COPY_ROOT="$TOOLS_COPY_PATH" \
      AGENT_TOOLS_VERSION="$TOOLS_VERSION" \
      AGENT_TOOLS_SOURCE="$tools_source_root" \
      direnv exec . "${cmd[@]}"
    else
      AGENT_WORKSPACE_ID="$workspace_id" \
      AGENT_WORKSPACE_PATH="$workspace_path" \
      AGENT_WORKSPACE_METADATA="$metadata_path" \
      AGENT_WORKSPACE_REPO_ROOT="$repo_root" \
      AGENT_TOOL_COPY_ROOT="$TOOLS_COPY_PATH" \
      AGENT_TOOLS_VERSION="$TOOLS_VERSION" \
      AGENT_TOOLS_SOURCE="$tools_source_root" \
      "${cmd[@]}"
    fi
  )
  exit_code=$?

  if [[ "$exit_code" == "0" ]]; then
    update_metadata "$metadata_path" "done"
  else
    update_metadata "$metadata_path" "error"
  fi

  if [[ "$cleanup" == "true" && "$exit_code" == "0" ]]; then
    jj workspace forget "$workspace_id"
    rm -rf "$workspace_path"
  fi

  return "$exit_code"
}

status_subcommand() {
  if [[ $# -gt 1 ]]; then
    fail "status accepts zero or one workspace id"
  fi

  if [[ $# -eq 1 ]]; then
    local workspace_id
    workspace_id=$(sanitise_workspace_id "$1")
    local metadata_path
    metadata_path=$(metadata_path_for "$workspace_id")
    if [[ ! -f "$metadata_path" ]]; then
      fail "workspace '$workspace_id' has no metadata at '$metadata_path'"
    fi
    python - "$metadata_path" <<'PY'
import json
import sys
path = sys.argv[1]
with open(path, 'r', encoding='utf-8') as fh:
    data = json.load(fh)
print(json.dumps(data, indent=2))
PY
    return 0
  fi

  python - "$workspace_repo_root" <<'PY'
import json
import os
import sys
from pathlib import Path

root = Path(sys.argv[1])
if not root.exists():
    print("No workspaces registered.")
    raise SystemExit(0)

rows = []
for child in sorted(root.iterdir()):
    if not child.is_dir():
        continue
    meta_path = child / '.agent-tools' / '.agent-workflow.json'
    if not meta_path.exists():
        continue
    try:
        data = json.loads(meta_path.read_text(encoding='utf-8'))
    except Exception:
        status = 'unknown'
        workflow = '-'
        updated = '-'
    else:
        status = data.get('status') or '-'
        workflow = data.get('workflow') or '-'
        updated = data.get('updated_at') or '-'
    rows.append((child.name, status, workflow, updated))

if not rows:
    print("No workspaces registered.")
    raise SystemExit(0)

name_width = max(len(r[0]) for r in rows)
print(f"{'WORKSPACE'.ljust(name_width)}  STATUS      WORKFLOW          UPDATED")
for name, status, workflow, updated in rows:
    print(f"{name.ljust(name_width)}  {status.ljust(10)}  {workflow.ljust(16)}  {updated}")
PY
}

shell_subcommand() {
  if [[ $# -ne 1 ]]; then
    fail "shell requires a workspace id"
  fi

  local workspace_id
  workspace_id=$(sanitise_workspace_id "$1")
  local workspace_path
  workspace_path=$(workspace_path_for "$workspace_id")
  [[ -d "$workspace_path" ]] || fail "workspace '$workspace_id' does not exist"

  if ! workspace_registered "$workspace_id"; then
    fail "workspace '$workspace_id' is not registered with jj"
  fi

  command -v direnv >/dev/null 2>&1 || fail "direnv is required but not installed"

  local shell_cmd
  shell_cmd=${SHELL:-/bin/sh}

  (
    cd "$workspace_path"
    direnv allow .
    echo "Attaching to workspace '$workspace_id' at '$workspace_path'." >&2
    exec direnv exec . "$shell_cmd" -i
  )
}

clean_subcommand() {
  if [[ $# -ne 1 ]]; then
    fail "clean requires a workspace id"
  fi

  local workspace_id
  workspace_id=$(sanitise_workspace_id "$1")
  local workspace_path
  workspace_path=$(workspace_path_for "$workspace_id")

  if workspace_registered "$workspace_id"; then
    jj workspace forget "$workspace_id"
  fi

  if [[ -d "$workspace_path" ]]; then
    case "$workspace_path" in
      "$workspace_repo_root"/*) ;;
      *) fail "refusing to remove path outside workspace cache: $workspace_path" ;;
    esac
    rm -rf "$workspace_path"
  fi
}

sync_tools_subcommand() {
  if [[ $# -ne 1 ]]; then
    fail "sync-tools requires a workspace id"
  fi

  local workspace_id
  workspace_id=$(sanitise_workspace_id "$1")
  local workspace_path
  workspace_path=$(workspace_path_for "$workspace_id")

  [[ -d "$workspace_path" ]] || fail "workspace '$workspace_id' does not exist"

  if ! workspace_registered "$workspace_id"; then
    fail "workspace '$workspace_id' is not registered with jj"
  fi

  prepare_tools_copy "$workspace_path" "true"

  local metadata_path
  metadata_path=$(metadata_path_for "$workspace_id")

  local prev_status="idle"
  local prev_workflow=""
  local prev_command="[]"
  local prev_direnv="false"
  local prev_base=""

  if [[ -f "$metadata_path" ]]; then
    local -a meta_info=()
    mapfile -t meta_info < <(python - "$metadata_path" <<'PY'
import json
import sys

path = sys.argv[1]
with open(path, 'r', encoding='utf-8') as fh:
    data = json.load(fh)

print(data.get('status', 'idle'))
print(data.get('workflow') or '')
print(json.dumps(data.get('command', [])))
print('true' if data.get('direnv_allowed') else 'false')
print(data.get('base_change') or '')
PY
    )
    if [[ ${#meta_info[@]} -ge 5 ]]; then
      prev_status="${meta_info[0]}"
      prev_workflow="${meta_info[1]}"
      prev_command="${meta_info[2]}"
      prev_direnv="${meta_info[3]}"
      prev_base="${meta_info[4]}"
    fi
  fi

  WORKSPACE_ID="$workspace_id"
  WORKSPACE_PATH="$workspace_path"
  METADATA_PATH="$metadata_path"
  WORKFLOW_NAME="$prev_workflow"
  BASE_CHANGE="$prev_base"
  COMMAND_JSON="$prev_command"
  DIRENV_ALLOWED="$prev_direnv"
  TOOLS_SOURCE="$tools_source_root"
  TOOLS_COPY="$TOOLS_COPY_PATH"
  TOOLS_VERSION="$TOOLS_VERSION"
  update_metadata "$metadata_path" "$prev_status"
}

main() {
  if [[ $# -lt 1 ]]; then
    usage
    exit 1
  fi

  local subcommand="$1"
  shift

  case "$subcommand" in
    run)
      [[ $# -ge 1 ]] || fail "run requires a workspace id"
      run_subcommand "$@"
      ;;
    status)
      status_subcommand "$@"
      ;;
    shell)
      shell_subcommand "$@"
      ;;
    clean)
      clean_subcommand "$@"
      ;;
    sync-tools)
      sync_tools_subcommand "$@"
      ;;
    *)
      usage
      fail "unknown subcommand '$subcommand'"
      ;;
  esac
}

main "$@"
