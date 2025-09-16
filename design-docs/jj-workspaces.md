# AI Workflow Workspaces

## Overview

Agent-driven workflows now execute inside dedicated Jujutsu workspaces. Each run gets
its own working copy outside of the repository root so that manual edits or parallel
workflows never step on each other. The helper script `scripts/agent-workspace.sh`
provisions these workspaces, keeps lightweight metadata, and guarantees the directory
is trusted by `direnv` so the nix environment loads automatically.

## Workspace Layout

- Root directory: `${AI_WORKSPACES_ROOT:-$XDG_CACHE_HOME/ai-workspaces}`.
- Repository namespace: `${basename(repo)}-${sha256(repo-root)[0..10]}` to avoid
  collisions between repos with the same name.
- Workspace path: `<root>/<repo-namespace>/<workspace-id>`.
- Metadata file: `.agent-workflow.json` within every workspace, recording the workflow
  name, status, timestamps, and the command that is running.

Workspaces are never created under the repository itself. This keeps the main tree
clean and prevents the permission issues we ran into when nesting workspaces inside
tracking directories.

## Helper Script Responsibilities

`scripts/agent-workspace.sh` centralises workspace management. Key behaviours:

- `run`: Create or reattach to the workspace, call `jj workspace add` if needed,
  optionally pin the workspace to a starting change, run `direnv allow`, and then
  execute the specified command via `direnv exec` so the nix shell is active.
- Metadata updates before and after the command capture runtime details. Failures are
  recorded as `status: "error"` for easier triage.
- `status`: Summarise all known workspaces or dump a single metadata file for inspection.
- `shell`: Attach an interactive shell to an existing workspace (after running
  `direnv allow`). This is handy for manual interventions mid-workflow.
- `clean`: Remove the workspace after telling Jujutsu to forget it.

Every command run inside a workspace receives environment variables describing where it
is running: `AGENT_WORKSPACE_ID`, `AGENT_WORKSPACE_PATH`, `AGENT_WORKSPACE_METADATA`,
`AGENT_WORKSPACE_REPO_ROOT`.

## Using the Workflows

- `just agents::consolidate <workspace-id> <start-change> <end-change>` creates or
  reuses a workspace and delegates the existing automation to `consolidate-inner`.
- Nested workflows should pass the same `workspace_id` down via `--set` so that every
  automated step stays inside the same working copy.
- `just agents::workspace-status` lists all workspaces for the repo. Add an ID to view
  the raw metadata, e.g. `just agents::workspace-status wf-123`.
- `just agents::workspace-shell <workspace-id>` opens an interactive, nix-enabled shell
  rooted at the workspace.
- `just agents::workspace-clean <workspace-id>` forgets the workspace in Jujutsu and
  removes the cached directory.

The helper does not auto-clean finished workspaces so that results can be inspected or
rebased manually. Once the work is integrated, run the cleanup recipe to delete the
working copy.
