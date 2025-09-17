# General Issues

## ISSUE-009
### Description
Earlier we planned to unify the list/sequence `lang_type` naming across both
recorders. The Rust tracer already emits `TypeKind::Seq` with the name
"List", and the pure-Python recorder continues to report "Array". With the
pure-Python implementation now deprecated, we only need to ensure the Rust
recorder stays aligned with the spec and that downstream consumers understand
the intentional divergence.

### Definition of Done
- Confirm the Rust recorder still emits `lang_type="List"` for Python lists.
- Document in the schema/README that the pure-Python recorder remains on
  "Array" because it is deprecated.
- Notify dependent fixtures/specs so they reference the Rust recorder as the
  source of truth.

### Proposed solution
- Add documentation clarifying the deprecation and accepted divergence; no
  changes to the pure-Python recorder are planned.

### Status
Backlog. Documentation-only follow-up; no recorder changes planned.


## ISSUE-010
### Description
Clarify scope of dict structural encoding and key typing. Product has decided
that every Python `dict` we record should be represented as a `Sequence` of
`(key, value)` tuples, regardless of where it appears. Keys that are not
strings must still be encoded using `encode_value` so that non-string keys do
not cause the recorder to fail. ISSUE-008 focused specifically on `**kwargs`,
but this decision applies across the board.

### Definition of Done
- Update both recorders (or confirm existing behavior) so that all dicts are
  encoded as sequences of `(key, value)` tuples, regardless of capture site.
- Keys are encoded through `encode_value` without special-casing strings or
  falling back to `repr`, and fixtures cover nested dicts and non-string keys.
- Docs/specs describe this behavior explicitly, including examples.
- Remove defensive fallbacks that conflict with the product decision.

### Proposed solution
- Standardize on the `(key, value)` tuple representation, relying entirely on
  `encode_value` for each component. Clean up the fallback logic so the encoder
  behaves consistently.

### Status
Low priority. We will not work on this until it blocks another issue or causes
user-reported problems.

## ISSUE-012
### Description
Extend the Rust-based recorder so it captures the values of local variables on
every traced step, allowing the UI to show how state evolves over time. Global
variable tracking will land in a follow-up (ISSUE-013). The pure-Python
recorder is deprecated and remains untouched, but we must document that clearly
in the repository.

We need a comprehensive test suite for the new behavior.

### Definition of Done
- The Rust recorder emits a full locals snapshot on each `LINE` event for
  functions, class bodies, comprehensions, and generator expressions.
- Locals are re-emitted on every step; we do not attempt to diff or elide
  unchanged values.
- Module-level `LINE` events also emit locals snapshots even though they mirror
  globals, so scripts that execute at import-time are fully captured.
- Example scripts are added under `/examples` to illustrate unfiltered locals
  capture and generator/coroutine suspension points (even though we only record
  on `LINE` events).
- Documentation outlines the new locals-capture behavior, clearly marks the
  pure-Python recorder as deprecated, states that module objects and
  `__builtins__` are filtered when possible (and documents any temporary
  limitations), and calls out that globals remain locals-only until ISSUE-013
  lands.
- Unit/integration tests cover representative scopes and ensure the existing
  `encode_value` usage is stable.
- Shipping scope is locals-only; document that global tracking is planned for
  ISSUE-013 and ensure tests/examples reinforce the current limitation.

### Design choices

* Target the Rust recorder only; the pure-Python implementation is deprecated.
* Rely on existing `runtime_tracing` hooks and capture locals for every traced
  scope, including non-function scopes.
* Continue to use `encode_value` as-is and only extend it to prevent crashes.
* Deliver a locals-only implementation first; defer global instrumentation to
  ISSUE-013 while documenting the gap.
* Do not filter by variable name—include dunder variables and function objects
  so product can review raw output.
* Exclude imported modules (`types.ModuleType`) and `__builtins__` from emitted
  locals snapshots whenever that filtering can be achieved without new
  instrumentation; when the filter is active, drop every module object rather
  than scoping by project root, and if we cannot enable the filter, document
  the limitation and track the follow-up.
* Capture module-level frames despite the duplication between `f_locals` and
  `f_globals`; global instrumentation will be addressed separately in
  ISSUE-013.
* Generator/coroutine yields do not require special events for now; examples
  should demonstrate the resulting traces.
* Global tracking and LOAD_GLOBAL/STORE_GLOBAL instrumentation is deferred to
  ISSUE-013.

### Further research
We can improve our idea how to implement the issue by looking at the following:
- Check ../codetracer-ruby-recorder which also tries to record the values but for Ruby. Maybe we can use some ideas from there.
- Check ../runtime_tracing to understand what capabilities the tracing library supports.
- Follow ISSUE-013 for the instrumentation required to track first-time global
  accesses within a scope.

### Status
In progress - blocked by ISSUE-015.

## ISSUE-013
### Description
Design the instrumentation the Rust recorder needs to start capturing global
variables only after a scope first touches them. We must detect
`LOAD_GLOBAL`/`STORE_GLOBAL` (and equivalents) so we can begin recording the
value without scanning every entry in `frame.f_globals`, while continuing to
skip builtins and imported modules.

### Definition of Done
- Specify how the recorder observes global accesses and mutations, including
  generator/coroutine scenarios.
- Outline the data we need from `runtime_tracing` or additional hooks, and add
  tasks/issues if the library requires changes.
- Define the cadence for emitting tracked globals, confirming they are
  re-emitted on every `LINE` event once observed.
- Document how we will avoid capturing builtins/imported modules while still
  emitting ordinary globals on every subsequent step.
- Provide tests/fixtures describing expected traces for new global tracking
  behavior.

### Design choices

* Detect first touches via instruction-level events so we avoid scanning
  `frame.f_globals` indiscriminately.
* Once a global is tracked within a scope, re-emit its value on every
  subsequent `LINE` event until the scope exits.
* Exclude builtins and imported modules from the tracked set even when they are
  accessed, and document the rationale for future revisit.

### Proposed solution
- Investigate instruction-level tracing (e.g., subscribing to LOAD/STORE
  events) and prototype a minimal detector that can toggle global capture per
  scope.
- Feed findings back into ISSUE-012 once ready for implementation.

### Status
Not started.

## ISSUE-014
### Description
Harden the public API for the Rust-backed recorder so that
`codetracer.start(format=...)` raises an explicit error when the requested
format is unsupported. The current fallback silently downgrades to binary
output, which hides configuration mistakes and diverges from the product
expectation.

### Definition of Done
- The Rust recorder validates the `format` argument and raises a descriptive
  error for unsupported values instead of silently changing the output format.
- Only `binary` and `json` (case-insensitive) are accepted; other values raise
  an error.
- Tests cover at least one supported format and a representative unsupported
  value to ensure the behavior stays stable.
- Public-facing docs mention the stricter error handling and list the accepted
  format values.

### Proposed solution
- Surface the validation as early as possible in `codetracer.start` so callers
  fail fast before tracing begins.
- Reuse or introduce a dedicated exception type shared with existing argument
  validation paths.

### Status
Backlog.

## ISSUE-015
### Description
Locals snapshots now call `encode_value` for every binding. When the fallback
path hits an object with a Python-defined `__str__`/`__repr__`, we execute user
code while monitoring is active. The first line event inside that `__str__`
re-enters `collect_locals`, which then tries to encode the same object again and
recurses until the process crashes. ISSUE-012 cannot ship until this is fixed.

### Definition of Done
- Pause monitoring (or otherwise guard re-entry) while encoding locals so nested
  `collect_locals` invocations cannot occur.
- Add regression coverage with a local whose `__str__` executes Python code.
- Document the reentrancy guard in the runtime tracer module.

### Proposed solution
- Use a per-thread reentrancy flag or temporarily disable monitoring around the
  locals snapshot encoding loop.

### Status
New.
