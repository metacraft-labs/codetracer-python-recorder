# Workflow Automation Platform Technical Specification

## 1. Background and Goals

The current proof of concept orchestrates AI-assisted workflows through ad-hoc `just`
recipes (`agents.just`) and a Bash helper script (`scripts/agent-workspace.sh`) that
manages per-run Jujutsu workspaces outside the repository tree.
To deliver a production-grade product we must transform this tooling into a cohesive,
extensible Rust platform that supports:

- User-authored workflows with first-class configuration, validation, and reuse.
- Publishing, discovering, and pulling shared workflows from a central registry.
- Safe parallel execution of multiple workflows using isolated workspaces.
- Operational observability, access control, and lifecycle management expected from a
  real product.

## 2. Product Requirements

### 2.1 Functional

1. **Workflow authoring** – Users can define workflows locally using a declarative file
   format (`workflow.toml`), including parameterized steps, dependencies, conditional
   execution, and reusable actions.
2. **Execution** – Users can run workflows locally via CLI, passing parameters and
   environment overrides. Runs use isolated workspaces derived from a Jujutsu change or
   repository head.
3. **Parallelization** – Users can queue or execute multiple workflows concurrently with
   per-run resource isolation and status tracking.
4. **Publishing** – Users can publish workflows (including metadata, version, license,
   and documentation) to a central registry service after authentication.
5. **Discovery & Fetching** – Users can search the registry, view metadata, download a
   workflow into their local cache, and upgrade to new versions.
6. **Introspection** – CLI and APIs expose run status, logs, workspace locations, and
   artifacts. Users can attach interactive shells to running workspaces.
7. **Lifecycle management** – Users can clean up workspaces, cancel runs, and configure
   retention policies for cached tooling and artifacts.

### 2.2 Non-Functional

- **Language** – Rust 2021 edition across all crates.
- **Portability** – Linux and macOS support (Windows optional, planned).
- **Security** – Signed workflow bundles, authenticated registry access, sandboxed step
  execution with configurable allow-lists.
- **Scalability** – Scheduler supports dozens of concurrent workflows on a single node;
  registry handles thousands of workflows and metadata queries.
- **Observability** – Structured logging, OpenTelemetry tracing, and metrics export.
- **Extensibility** – Pluggable actions and custom step types without recompiling the
  core runtime.

## 3. System Architecture Overview

The platform is organized into cooperating Rust crates/services:

- `workflow-core` – Parsing, validation, and planning for workflow definitions.
- `workspace-manager` – Rust port of workspace provisioning, replacing
  `agent-workspace.sh` while preserving the features documented in the workspace design
  doc.
- `workflow-executor` – Asynchronous engine that schedules workflow graphs, creates
  workspaces, executes steps, and collects results.
- `workflow-cli` – End-user interface built with `clap`, orchestrating local commands.
- `workflow-registry-server` – Central service exposing REST/JSON APIs for workflow
  publish, fetch, and discovery.
- `workflow-registry-client` – Shared library used by CLI/executor to interact with the
  registry, manage auth tokens, and cache bundles.
- `workflowd` (optional) – Long-running daemon that the CLI can delegate to for
  background execution and concurrency control on shared machines.

Core data flow:

1. User invokes `workflow run <workflow-id>` (local or fetched). CLI loads definition via
   `workflow-core`, resolves dependencies, and submits run to `workflowd` or local
   executor.
2. Executor requests workspace allocation from `workspace-manager`, which creates or
   reuses a workspace, copies tooling, and returns metadata.
3. Executor schedules steps according to DAG dependencies, running actions (shell,
   built-in Rust code, or plugin) inside the workspace with environment variables
   matching the current proof of concept.
4. Run state, logs, and artifacts stream to local storage; optional upload to registry or
   external artifact store.
5. CLI polls or receives events to display progress. Upon completion, metadata is
   persisted; optional cleanup occurs per policy.

## 4. Component Specifications

### 4.1 Workflow Definition Format (`workflow.toml`)

- **Schema**
  - `id` (string, semantic version optional) and `name`.
  - `description`, `tags`, `license`, `homepage`.
  - `parameters` (name, type, default, required, validation regex).
  - `artifacts` declarations (path patterns, retention policy).
  - `steps`: map from step id → struct with `uses` (action reference), `inputs`,
    `env`, `run` (command), `needs` (dependencies), `when` (expression), and
    `workspace` (inherit, ephemeral, or custom path).
  - `actions`: reusable step templates referencing built-in adapters or external
    binaries.
  - `requirements`: toolchain prerequisites (e.g., nix profile, docker image, python
    packages) for validation before run.
- **Parser** – Implemented with `serde` + `toml`. Provide JSON schema export for editor
  tooling.
- **Validation** – Ensure DAG acyclicity, parameter resolution, and compatibility with
  workspace policies. Emit actionable diagnostics.
- **Extensibility** – Support plugin-defined parameter types and validators via dynamic
  registration.

### 4.2 `workflow-core` Crate

- Modules:
  - `model` – Rust structs representing workflows, steps, actions, parameters.
  - `parser` – Functions to load from file/URL, merge overrides, and surface line/column
    errors.
  - `validator` – Graph validation, parameter type checking, requirement resolution.
  - `planner` – Convert workflow definitions + runtime parameters into executable DAG
    plans with resolved command strings and environment.
- Exposes stable API consumed by CLI, executor, and registry server.
- Provides `serde` serialization for storing compiled plans in the registry.
- Includes unit tests covering parsing edge cases, invalid graphs, and parameter
  substitution.

### 4.3 `workspace-manager` Crate

- Reimplements responsibilities of `agent-workspace.sh` in Rust:
  - Manage workspace root discovery, hashed repo namespace, and metadata persistence in
    `.agent-tools/.agent-workflow.json`.
  - Provide APIs: `ensure_workspace(id, base_change, direnv_policy)`,
    `update_status(status, workflow, command)`, `cleanup(id)`, `sync_tools(id)`.
  - Copy automation bundles (`agents.just`, `scripts/`, `rules/` by default) into
    `.agent-tools/`, hashing contents to avoid redundant copies.
  - Execute commands via `direnv exec` when available; fall back to plain execution if
    disabled.
  - Emit structured events for workspace lifecycle (created, reused, direnv allowed,
    tooling hash, cleanup).
- Implementation details:
  - Use `tokio::process` for subprocess management.
  - Use `serde_json` for metadata file compatibility with current schema.
  - Provide CLI subcommands reused by `workflow-cli` for manual inspection.

### 4.4 `workflow-executor` Crate

- Built atop `tokio` runtime with cooperative scheduling.
- Responsibilities:
  - Accept execution plans from `workflow-core`.
  - Allocate workspaces per workflow run or per-step when `workspace = "ephemeral"`.
  - Manage concurrency using per-run DAG scheduler; configurable max parallel steps.
  - Execute actions:
    - **Shell command**: spawn process with inherited/captured stdio; enforce timeouts
      and environment.
    - **Built-in adapters**: native Rust functions implementing actions like `jj diff`
      summarization.
    - **Plugins**: load dynamic libraries (`cdylib`) conforming to trait `ActionPlugin`.
  - Collect logs, exit codes, produced artifacts; stream to observers.
  - Handle cancellation, retries, backoff, and failure propagation (fail-fast or
    continue modes per step).
- Provides event stream (`RunEvent`) consumed by CLI/daemon for status updates.
- Maintains run metadata store (SQLite via `sqlx` or `rusqlite`) capturing history and
  enabling queries.

### 4.5 `workflow-cli` Crate

Commands (subset):

- `workflow init` – Scaffold new `workflow.toml` with templates.
- `workflow validate [file]` – Run parser and validator.
- `workflow run <id|path> [--param key=value] [--workspace-id ...] [--parallel N]` –
  Execute workflows, optionally delegating to daemon.
- `workflow status [run-id]` – Show active runs, including workspace paths and metadata.
- `workflow logs <run-id>` – Stream logs and step outputs.
- `workflow workspace <list|show|shell|clean>` – User-facing wrappers around
  `workspace-manager` operations.
- `workflow publish <path> [--registry]` – Package and upload to registry.
- `workflow fetch <workflow-ref>` – Download to local cache.
- `workflow registry login` – Acquire/store auth token securely (OS keyring).

Implementation notes:

- Built with `clap` derive, asynchronous commands using `tokio`.
- CLI communicates with daemon via Unix domain socket/gRPC (tonic) when running in
  background mode; falls back to in-process executor.
- Provides colored terminal UI (indicatif) for progress bars and summary tables.

### 4.6 `workflow-registry-server`

- Rust service built with `axum` + `tower`.
- Stores workflow bundles (TOML + optional assets) and metadata in PostgreSQL or SQLite.
- REST API endpoints:
  - `POST /v1/workflows` – Publish new version (requires auth, accepts signed tarball).
  - `GET /v1/workflows` – Search by tag, owner, text.
  - `GET /v1/workflows/{id}` – Fetch metadata and available versions.
  - `GET /v1/workflows/{id}/{version}/download` – Stream bundle.
  - `PUT /v1/workflows/{id}/{version}/deprecate` – Mark version as deprecated.
  - `GET /v1/tags` – Enumerate tags/categories.
- Authentication via OAuth2 access tokens or PAT; integrate with identity provider.
- Supports content-addressed storage (CAS) for deduplicated bundles (S3-compatible
  backend optional).
- Provides audit logs and signed metadata (Ed25519). Server verifies bundle signature
  and publishes signature chain for clients.

### 4.7 `workflow-registry-client`

- Shared crate handling:
  - Auth token storage and refresh.
  - HTTP client (reqwest) with retry/backoff, TLS pinning optional.
  - Local cache of downloaded bundles under `$XDG_CACHE_HOME/workflows/<id>/<version>`.
  - Signature verification before unpacking.
  - Integration with CLI/executor to auto-update cached workflows.

### 4.8 `workflowd` Daemon (Optional but recommended)

- Runs locally as background service; manages queue of workflow runs and enforces
  concurrency limits.
- Exposes control API over gRPC/Unix socket: submit run, stream events, cancel, list
  runs, attach shell (spawn using `workspace-manager`).
- Persists state in local SQLite to survive restarts.
- Implements cooperative scheduling across workflows, respecting per-user and global
  limits.

### 4.9 Observability & Telemetry

- Unified logging via `tracing` crate with JSON output option.
- Emit OpenTelemetry spans for major operations (parsing, workspace allocation, step
  execution) with context propagation from CLI to daemon to registry calls.
- Metrics (Prometheus exporter) for run success rates, queue depth, workspace lifecycle.
- Artifact metadata includes checksums and retention metadata for cleaning policies.

### 4.10 Packaging and Distribution

- Provide `cargo` workspace with crates listed above; enable `--features daemon` etc.
- Offer standalone binaries via `cargo dist` or `nix` flake integration.
- Provide container image for registry server and optional `workflowd`.
- Ensure integration with existing `just` recipes for compatibility during migration.

## 5. Parallel Execution & Scheduling

- Scheduler maintains run queue prioritized by submission time and priority class.
- Per-run concurrency derived from workflow definition; defaults to sequential.
- Implement resource leasing to avoid oversubscribing CPU/memory; allow configuration
  via CLI/daemon.
- Guarantee workspace uniqueness per run; share read-only caches (e.g., tool bundles)
  to minimize duplication.
- Provide cancellation tokens; steps respond promptly to interrupts.

## 6. Security & Permissions

- Workflow bundles signed with user-specific keys; registry verifies signatures.
- CLI validates signatures and optionally enforces allow-list for publishers.
- Sandboxed execution options:
  - Support running steps inside container runtimes (e.g., `nix develop`, `podman`).
  - Provide file access policies per workspace (readonly host repo except workspace
    copy).
- Secrets management: CLI loads env secrets from OS keyring or `.env` with opt-in.
- Auditing: persist run metadata (who ran what, when, with which workflow version).

## 7. Testing and Quality Strategy

- Unit tests in each crate; property tests for parser and planner.
- Integration tests using temporary repositories and mocked registry server.
- End-to-end tests executed via `cargo nextest` that run sample workflows through CLI
  and executor using fixture registry data.
- Provide smoke-test command `workflow self-test` to validate installation.

## 8. Migration from Proof of Concept

1. Implement `workspace-manager` crate mirroring Bash functionality, validated against
   scenarios in `agent-workspace.sh` (run, status, shell, clean, sync-tools).
2. Port representative workflows from `agents.just` into `workflow.toml` definitions to
   ensure feature parity (workspace-aware steps, iterative loops, interactive shells).
3. Wrap existing `just` recipes to call new CLI for backward compatibility during
   transition period.
4. Deprecate Bash script once Rust manager is stable; mark `agents.just` workflows as
   legacy and document migration path.

## 9. Open Questions

- Should the workflow format support embedded Python/Rust scripts, or require external
  files?
- How do we support long-running interactive steps (e.g., human-in-the-loop) within the
  DAG while preserving resumability?
- What identity provider(s) should the registry integrate with, and do we need
  fine-grained ACLs per workflow?
- Do we require distributed execution (multi-machine) in the initial release, or is
  single-host parallelism sufficient?
- How should artifact storage integrate with external systems (S3, OCI registries)?

