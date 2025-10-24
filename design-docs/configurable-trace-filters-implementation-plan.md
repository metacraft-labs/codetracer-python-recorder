# Configurable Trace Filters – Implementation Plan

Plan owners: codetracer recorder maintainers  
Target PRD: US0028 – Configurable Python trace filters  
Related ADR: 0009 – Configurable Trace Filters for codetracer-python-recorder

## Goals
- Load one or more TOML filter files (`filter_a::filter_b`) and compile them into a reusable engine that models ordered scope rules and value redaction patterns.
- Gate tracing for packages, files, and qualified objects before we allocate call/line events. Redact locals, globals, args, and return payloads while keeping variable names visible.
- Expose configuration through the Python API, CLI, and environment variables with actionable validation errors.
- Preserve performance: cached decisions keep `on_py_start`, `on_line`, and `on_py_return` within the existing overhead budget (<10 % slowdown and <8 µs added per event when filters are active).

## Performance Targets
- **First-run resolution:** <20 µs to resolve a new code object against 50 scope rules (single-thread median).
- **Steady-state callbacks:** <4 µs extra per event for value policy checks on frames with ≤10 variables.
- **Memory overhead:** <200 KB for 1 000 cached code-object resolutions plus compiled matchers.
- **Regression threshold:** Alert if end-to-end trace runtime increases by >10 % compared to the baseline with filters disabled.

## Current Gaps
- `RuntimeTracer::should_trace_code` (`src/runtime/mod.rs:495`) only checks for synthetic filenames; it cannot honour include/exclude lists or precedence.
- Value capture helpers (`src/runtime/value_capture.rs:14-110`) always encode full values; there is no redaction path.
- `RecorderPolicy` (`src/policy.rs`) has no filter state, and the session bootstrap never looks for `.codetracer/trace-filter.toml`.
- The Python facade (`codetracer_python_recorder/session.py`) and CLI lack flags for supplying filter files.
- No tests exercise filtered traces or redaction semantics.

## Workstreams

### WS1 – Selector Parsing & Compilation
**Scope:** Build the shared selector infrastructure that understands both scope and value patterns.
- Add `toml`, `globset`, and `regex` dependencies in `Cargo.toml`.
- Create `src/trace_filter/selector.rs` with:
  - `SelectorKind` & `MatchType` enums covering `pkg`, `file`, `obj`, `local`, `global`, `arg`, `ret`, `attr`.
  - `Selector` struct storing original text plus compiled matcher (`GlobMatcher`, `Regex`, or literal string).
  - Public `Selector::parse(raw: &str, permitted: &[SelectorKind]) -> RecorderResult<Selector>`.
- Unit tests in `src/trace_filter/selector.rs` for glob, regex, literal, invalid kind, missing pattern, and reserved kinds.
- Exit criteria: `cargo test selector` (module tests) passes; parsing rejects malformed selectors with `ERR_INVALID_POLICY_VALUE`.

### WS2 – Filter Model & Loader
**Scope:** Parse TOML files and resolve ordered scope/value rules with inheritance and composition.
- Add `src/trace_filter/config.rs` defining serde models that mirror the doc schema:
  - `TraceFilterFile` (meta, scope defaults, rule arrays).
  - Deny unknown keys, supply precise error context (filename + table path).
- Implement loader API:
  - `TraceFilterConfig::from_paths(paths: &[PathBuf]) -> RecorderResult<TraceFilterConfig>`.
  - Resolve `inherit` by walking the composed chain left-to-right.
  - Produce a flattened `Vec<ScopeRule>` where each rule carries `exec`, `value_default`, and `Vec<ValuePattern>`.
  - Store per-file project root (parent of `.codetracer`) to normalise `file` selectors (relative POSIX paths) and derive module names.
- Add helper to serialise the active chain for metadata (`FilterSummary` with absolute path + SHA256 digest).
- Unit tests using temp files covering:
  - Successful parse with defaults, appended rules, and value pattern inheritance.
  - Error on unknown keys, missing selector, invalid enum values, or circular inherit (inherit without base).
  - Path normalisation for `file` selectors (dot-join, `__init__.py` -> package).
- Exit criteria: `cargo test trace_filter` covers loader error paths; composition order matches spec.

### WS3 – Runtime Engine & Caching
**Scope:** Evaluate filters in hot callbacks without repeated string matching.
- Design `TraceFilterEngine` in `src/trace_filter/engine.rs`:
  - Hold shared `Arc<[ScopeRule]>` from WS2.
  - Provide `resolve(py, code: &CodeObjectWrapper) -> RecorderResult<ScopeResolution>` caching results per code id (HashMap inside engine or `RuntimeTracer`).
  - `ScopeResolution` contains `exec: ExecDecision`, `value_policy: ValuePolicy`, and metadata (module name, path, matched rule index for debugging).
- Module derivation:
  - When the absolute filename sits under the filter’s project root, compute relative module (`pkg`) and qualified object (`module.qualname`).
  - Fallback to using globals `__name__` once per code when the frame snapshot becomes available; store result in cache.
- Add telemetry counters (using `log::debug!` + `record_dropped_event`) when rules trigger skips or redactions.
- Unit tests (mock `CodeObjectWrapper` via Python) verifying:
  - Per-code caching (second call does not re-evaluate selectors).
  - File selector matches relative path; unmatched files fall back to defaults.
  - Object selector precedence beats package/file when ordered later.
- Exit criteria: `cargo test trace_filter::engine` passes; flamegraph on synthetic benchmark shows <2 µs overhead per decision.

### WS4 – RuntimeTracer Integration
**Scope:** Apply execution and value policies during tracing.
- Extend `RuntimeTracer::new` signature to accept `Option<Arc<TraceFilterEngine>>`; store in a new field plus `HashMap<usize, ScopeResolution>`.
- Update `should_trace_code` to consult the cached resolution:
  - If `Skip`, record `ignored_code_ids` as today, increment `filter.skipped_scopes`.
  - If `Trace`, fall through.
- Modify `capture_call_arguments` & `record_visible_scope` to take `ValuePolicy` and return redacted `FullValueRecord` when denial occurs (use helper `redacted_value(writer)`).
- Add `ValueKind` enum for locals/globals/args/return; implement match helper `ValuePolicy::decide(kind, name)`.
- Adjust `record_return_value` to apply policy; still emit event with sentinel when denied.
- Ensure `ValuePolicy` respects ordered `value_patterns`, falling back to default; add instrumentation counters.
- Update unit/integration tests in `src/runtime/mod.rs`:
  - A script with two functions; filter to skip one and redact variables in the other.
  - Assert `line_snapshots` ignore skipped code ids; returned trace contains `<redacted>` markers.
- Exit criteria: `cargo test -p codetracer-python-recorder` passes; new tests enforce skip + redaction semantics.

### WS5 – Python Surface, CLI, and Metadata
**Scope:** Wire filters through session helpers and document them.
- Update `#[pyfunction] start_tracing` signature with `#[pyo3(signature = (path, format, activation_path=None, trace_filter=None))]`.
  - Parse `trace_filter` (string/path) into `FilterSpec`, split on `::`, resolve to absolute paths, and feed into loader. Map errors via `RecorderError`.
- Extend `TraceSessionBootstrap` (or adjacent helper) to find the default `<project>/.codetracer/trace-filter.toml` by walking up from the script path when no explicit spec is provided.
- Prepend a built-in default filter (shipped with the crate) that redacts common secrets and skips standard-library/asyncio frames before applying project/user filters.
- Modify `session.start` and `.trace` to accept `trace_filter` keyword; wrap `pathlib.Path` inputs.
- CLI:
  - Add `--trace-filter path` (repeatable). When multiple provided, respect CLI order; combine with default using `::`.
  - Show helpful message when file missing or parse fails.
- Auto-start: read `CODETRACER_TRACE_FILTER`.
- Augment trace metadata writer (`TraceOutputPaths::write_metadata` or equivalent) with filter summary (paths + hashes).
- Python tests:
  - Unit test for CLI parsing with `--trace-filter baseline --trace-filter local`.
  - Integration test running sample script with filter toggles to ensure skip + redaction propagate end-to-end.
- Exit criteria: `just test` passes; CLI help documents new flag; metadata includes filter summary.

### WS6 – Hardening, Benchmarks & Documentation
**Scope:** Final polish, monitoring, and rollout artefacts.
- Add microbench harness (`cargo bench` or `criterion`) that runs a synthetic workload (10 k function calls, 50 locals) twice: filters disabled vs enabled with representative rule sets (glob-heavy and regex-heavy). Collect mean/median latency per callback and total runtime.
- Integrate a Python smoke benchmark (`pytest -k test_filter_perf`) that executes a real script via `TraceSession` to capture cross-language overhead.
- Fail CI when slowdown >10 % or absolute time exceeds targets; emit perf summaries in logs.
- Add logging guard for regex compilation failures with actionable remediation.
- Update README + docs (`docs/` tree) with filter syntax, examples, env vars, CLI usage.
- Create status tracker `configurable-trace-filters-implementation-plan.status.md`.
- Coordinate security review for default secret redaction patterns.
- Exit criteria: Benchmarks recorded and documented, performance dashboards show compliance with targets, documentation merged, ADR 0009 moves to **Accepted** after WS1–WS6 merge.

## Verification Strategy
- Unit tests per module (`trace_filter::selector`, `trace_filter::config`, `trace_filter::engine`, runtime integration).
- Python integration tests verifying CLI + API end-to-end filtering.
- Manual smoke: run `python -m codetracer_python_recorder --trace-filter examples/filters/dev.toml examples/sample_app.py`.
- CI: extend `just test` to include new Rust + Python suites; add lint ensuring `<redacted>` sentinel constant stays consistent.

## Risks & Mitigations
- **Performance regression:** Mitigated by caching `ScopeResolution`, precompiling globs/regex, and benchmarking before release.
- **Configuration errors causing silent allow:** Strict TOML schema + explicit `inherit` validation prevents silent fallback; we surface `RecorderError` with file + line context.
- **Path derivation mismatch on Windows:** Normalise paths using `Path::components` and always convert to forward slashes before glob matching. Include cross-platform tests via CI.
- **Regex denial-of-service:** Document recommended anchors, enforce a maximum length (e.g., 512 characters) during parse, and reject overly complex patterns with clear errors.

## Timeline & Sequencing
1. WS1–WS2 can land together behind a feature flag.
2. WS3 depends on the loader; start once parsing tests pass.
3. WS4 (runtime wiring) and WS5 (surface) should land on the same feature branch to keep Python/Rust in sync.
4. WS6 wraps rollout, docs, and benchmarks before flipping the feature flag on by default.
