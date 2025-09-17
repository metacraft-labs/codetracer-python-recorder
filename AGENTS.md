# Instructions for Codex

This repository contains two related projects:

- codetracer-pure-python-recorder — the original pure-Python tracer.
- codetracer-python-recorder — a Rust-backed Python module built with PyO3 and maturin.

To build the modules in development mode run:

```sh
just venv 3.13 dev #You can use any other Python version >=3.12
``

Then to run the tests do

```sh
just test
```

## Agent Workspaces

Automation now runs inside dedicated Jujutsu workspaces that live outside of the
repository tree. Use the helper recipes to inspect and control them:

- `just agents::consolidate <workspace-id> <start-change> <end-change>` – run the
  consolidate workflow inside a workspace. The workspace is trusted with `direnv`
  so the nix environment loads automatically.
- `just agents::workspace-status [<workspace-id>]` – list all workspaces for this
  repository or show metadata for a single workspace.
- `just agents::workspace-shell <workspace-id>` – attach an interactive shell to a
  workspace (the environment is prepared with `direnv allow`).
- `just agents::workspace-clean <workspace-id>` – forget the workspace and delete
  its cached directory once the work is integrated.
- `just agents::workspace-sync-tools <workspace-id>` – refresh the copied automation
  bundle inside a workspace without re-running the workflow.

Workspaces are stored under `${AI_WORKSPACES_ROOT:-$XDG_CACHE_HOME/ai-workspaces}`
using a repository-specific namespace. Each workspace contains a `.agent-tools/`
directory with the current automation (`agents.just`, `scripts/`, `rules/`). The helper
copies these files before every run so workflows see the latest tooling even when the
target change is older. See `design-docs/jj-workspaces.md` for full rationale and
lifecycle details.

# TOOLING

Instead of writing shell commands directly you should use `just`
commands in the `ai` module. That is run commands like `just
ai::COMMAND`.  The commands that you should use are described in
`ai.just`. If a useful command is missing you should add it to the
just file.


# Code quality guidelines

- Strive to achieve high code quality.
- Write secure code.
- Make sure the code is well tested and edge cases are covered. Design the code for testability.
- Write defensive code and make sure all potential errors are handled.
- Strive to write highly reusable code with routines that have high fan in and low fan out.
- Keep the code DRY.
- Aim for low coupling and high cohesion. Encapsulate and hide implementation details.

# Code commenting guidelines

- Document public APIs and complex modules.
- Maintain the comments together with the code to keep them meaningful and current.
- Comment intention and rationale, not obvious facts. Write self-documenting code.
- When implementing specific formats, standards or other specifications, make sure to
  link to the relevant spec URLs.

# Writing git commit messages

The first line of the commit message should follow the "conventional commits" style:
https://www.conventionalcommits.org/en/v1.0.0/

In the remaining lines, provide a short description of the implemented functionality.
Provide sufficient details for the justification of each design decision if multiple
approaches were considered.
