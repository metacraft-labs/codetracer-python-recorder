# ADR 0009: Configurable Trace Filters for codetracer-python-recorder

- **Status:** Proposed
- **Date:** 2025-10-11
- **Deciders:** codetracer recorder maintainers
- **Consulted:** DX tooling crew, Privacy review group
- **Informed:** Replay consumers, Support engineering

## Context
- The PyO3 recorder (`src/runtime/mod.rs`) traces every code object whose filename looks "real" and captures all locals, globals, call arguments, and return values without any policy gate.
- `RecorderPolicy` (`src/policy.rs`) only controls error behaviour, logging, and IO capture. There is no notion of user-authored trace filters or redaction rules.
- The user story *US0028 – Configurable Python trace filters* mandates a unified selector DSL covering packages, files, and code objects plus value-level allow/deny lists processed in declaration order.
- The original pure-Python tracer has no reusable filtering engine we can transplant; the Rust backend needs its own parser, matcher, and runtime integration.
- Tracing hot paths (`on_py_start`, `on_line`, `on_py_return`) must stay cheap. We already cache `CodeObjectWrapper` attributes and blacklist synthetic filenames via `ignored_code_ids`.

## Problem
We must let maintainers author deterministic filters that:
- Enable or disable tracing for specific packages, files, or fully qualified code objects with glob/regex support.
- Allow or redact captured values (locals, globals, arguments, return payloads) per scope while keeping variable names visible.
- Compose multiple filter files (`baseline::overrides`) with predictable default inheritance.

The solution has to load human-authored TOML, enforce schema validation, and add minimal overhead to the monitoring callbacks. Policy errors must surface as structured `RecorderError` instances.

## Decision
1. **Introduce a `trace_filter` module (Rust)** compiling filters into an immutable `TraceFilterEngine`.
   - Parse TOML using `serde` + `toml` with `deny_unknown_fields`.
   - Support the selector grammar `<kind> ":" [<match_type> ":"] <pattern>` for both scope rules (`pkg`, `file`, `obj`) and value patterns (`local`, `global`, `arg`, `ret`, `attr`).
   - Compile globs with `globset::GlobMatcher`, regexes with `regex::Regex`, and literals as exact byte comparisons. Keep compiled matchers alongside original text for diagnostics.
   - Resolve `inherit` defaults while chaining multiple files (split on `::`). Later files append to the ordered rule list; `value_patterns` are likewise appended.
2. **Expose filter loading at session bootstrap.**
   - Extend `TraceSessionBootstrap` to locate the default project filter (`<cwd>/.codetracer/trace-filter.toml` up the directory tree) and accept optional override specs from CLI, Python API, or env (`CODETRACER_TRACE_FILTER`).
   - Prepend a bundled `builtin-default` filter that redacts common secrets and skips CPython standard-library/asyncio frames before applying project/user filters.
   - Parse each provided file once per `start_tracing` call. Propagate `RecorderError` on IO or schema failures with context about the offending selector.
3. **Wire the engine into `RuntimeTracer`.**
   - Store `Arc<TraceFilterEngine>` plus a per-code cache of `ResolvedScope` decisions (`HashMap<usize, ScopeResolution>`). Each resolution records:
     - Final execution policy (`Trace` or `Skip`).
     - Effective value default (`Allow`/`Deny`).
     - Ordered `ValuePattern` matchers ready for evaluation.
   - Update `should_trace_code` to consult the cache. A `Skip` result adds the code id to `ignored_code_ids` so PyO3 disables future callbacks for that location.
   - Augment `capture_call_arguments`, `record_visible_scope`, and `record_return_value` to accept a `ValuePolicy`. Encode real values for `Allow` and emit a reusable redaction record (`ValueRecord::Error { msg: "<redacted>" }`) for `Deny`.
   - Preserve variable names even when redacted; mark redaction hits via diagnostics counters so we can surface them later.
4. **Surface configuration from Python.**
   - Extend `codetracer_python_recorder.session.start` with a `trace_filter` keyword accepting a string or pathlike. Accept the same parameter on the CLI as `--trace-filter`, honouring `filter_a::filter_b` composition or repeated flags.
   - Teach the auto-start helper to respect `CODETRACER_TRACE_FILTER` with the same semantics.
   - Provide `codetracer_python_recorder.codetracer_python_recorder.configure_trace_filter(path_spec: str | None)` to preload/clear filters for embedding scenarios.
5. **Diagnostics and metadata.**
   - Record the active filter chain in the trace metadata header (list of absolute file paths plus a hash of each) so downstream tools can reason about provenance.
   - Emit structured redaction counters (e.g., `filter.redactions.locals`, `filter.skipped_scopes`) through the existing logging channel at debug level.

## Consequences
- **Upsides:** Maintainers gain precise control over tracing scope and redaction without touching runtime code. Ordered evaluation keeps behaviour predictable, and caching ensures hot callbacks only pay a fast hash lookup.
- **Costs:** Startup becomes more complex (reading and compiling TOML, glob/regex dependencies). We must carefully validate user input and provide actionable errors. RuntimeTracer grows extra state and branching, requiring new tests to guard regressions.
- **Risks:** Incorrect module/file derivation could lead to unexpected matches; we'll derive package names from relative paths and cache results to minimise repeated filesystem work. Regex filters can be expensive; precompilation mitigates per-event cost, but we still need guardrails against runaway patterns (document best practices, potentially add a length cap).

## Alternatives
- **Keep filters in Python.** Rejected because value capture happens in Rust; Python-driven filters would require round-tripping locals and arguments across the FFI, negating performance and privacy benefits.
- **Embed YAML/JSON instead of TOML.** TOML matches the existing design doc examples, integrates well with `serde`, and offers comments—preferred for hand-authored configs.
- **Per-event dynamic evaluation without caching.** Discarded due to hot-path overhead; caching `ScopeResolution` by code id keeps callbacks cheap while still honouring ordered overrides.

## Rollout
1. Land the parser, engine, and RuntimeTracer integration behind a feature flag (e.g., `trace-filters`) defaulting on once unit + integration tests pass.
2. Update CLI and Python APIs together so downstream consumers see a coherent interface.
3. Ship documentation and sample filters, then flip ADR status to **Accepted** after verifying the implementation plan milestones.
4. Monitor performance regressions via benchmarks that stress argument/local capture with filters enabled vs disabled. Adjust caching or selector matching if overhead exceeds the 10 % guardrail.

## Performance Analysis
- **Baseline hot paths:** `RuntimeTracer::on_py_start`, `on_line`, and `on_py_return` currently perform bounded work—lookup cached `CodeObjectWrapper` metadata, encode locals/globals once per event, and write to `NonStreamingTraceWriter`. Filtering adds (a) a first-use compilation pass per `code.id()` and (b) per-value policy checks.
- **First-use resolution:** When a new code object appears we compute `{package, file, qualname}` and walk the ordered scope list. With precompiled matchers the dominant cost is string comparison and glob/regex evaluation. Even with 50 rules the resolution remains under ~20 µs on a 3.4 GHz CPU (one hash lookup plus a few matcher calls). Result caching (hash map keyed by `code.id()`) ensures the cost is paid once per code object.
- **Per-event overhead:** After resolution we only pay a pointer lookup to fetch the cached `ScopeResolution`. Value capture walks the small `value_patterns` vector (expected count <10) until a match is found. Redaction emits a constant `ValueRecord::Error` without allocating large buffers. In aggregate this adds ~200–400 ns per variable inspected; for a typical frame with 5 locals and 4 arguments we expect <4 µs extra per event.
- **Memory impact:** The engine retains compiled matchers (`GlobMatcher`, `Regex`), rule metadata, and cached decisions. With 1 000 functions the per-code cache stores ~96 bytes each (decision enum, `Arc<ValuePolicy>`, module/path strings). Total footprint stays well below 200 KB in realistic projects.
- **Mitigations:**
  - Compile regexes/globs at load time and reject unanchored patterns longer than 512 bytes to avoid pathological backtracking.
  - Normalise filenames/modules once per code object; cache derived module names inside `ScopeResolution`.
  - Use `SmallVec<[ValuePattern; 4]>` (or similar) to keep pattern vectors stack-backed for the common case.
  - Reuse a static redaction sentinel (`<redacted>`) to avoid allocating per denial.
- **Benchmark strategy:** Extend the existing microbench harness to execute a synthetic script (10 k function calls, 50 locals) with filters disabled vs enabled. Capture total wall-clock time per callback and report delta. Alert when slowdown exceeds 10 % or absolute cost surpasses 8 µs per event. Include a variant with heavy regex patterns to ensure guardrails hold.
- **Continuous monitoring:** Emit debug counters (`filter.skipped_scopes`, `filter.redactions.*`) and plumb them into `RecorderMetrics` so we can spot rules that trigger excessively, potentially indicating misconfigured filters that inflate overhead.
