# General Issues

## ISSUE-009
### Description
Unify list/sequence `lang_type` naming across recorders. The Rust tracer now
emits `TypeKind::Seq` with name "List" for Python `list`, while the
pure-Python recorder uses "Array". This divergence can fragment the trace
schema and complicate downstream consumers.

### Definition of Done
- Both recorders emit the same `lang_type` for Python list values.
- Fixtures and docs/spec are updated to reflect the chosen term.
- Cross-recorder tests pass with consistent types.

### Proposed solution
- We will use "List" in order to match existing Python nomenclature

### Status
Low priority. We won't work on this unless it blocks another issue.


## ISSUE-010
### Description
Clarify scope of dict structural encoding and key typing. The current change
encodes any Python `dict` as a `Sequence` of `(key, value)` tuples and falls
back to generic encoding for non-string keys. Repo rules favor fail-fast over
defensive fallbacks, and ISSUE-008 focused specifically on `**kwargs`.

### Definition of Done
- Decide whether structural dict encoding should apply only to kwargs or to all
  dict values; document the choice.
- If limited to kwargs, restrict structured encoding to kwargs capture sites.
- If applied generally, define behavior for non-string keys (e.g., fail fast)
  and add tests for nested and non-string-key dicts.

### Proposed solution
- Prefer failing fast on non-string keys in contexts representing kwargs; if
  general dict encoding is retained, update the spec and tests and remove the
  defensive fallback for key encoding.

### Status
Low priority. We won't work on this until a user reports that it causes issues.

## ISSUE-012
### Description
Record values of local and global variables with the recorder.

We need to store the state and be able to track how it changes over time.

We need a comprehensive test suite for our solution.

### Design choices

* Current runtime_tracing implementation requires that we store all
  variables at each step. The variables which we record after a given
  step will be what is shown in the UI when we are at this step.
  
* This means that at each step we need to find out ALL variables which
  are accessible at the step. Then we need to encode each one using
  our value encoder. The value encoder is currently very basic, we
  will improve it as a part of a separate issue. However we don't
  intend to encode full values, we will be forced to sample the
  values.

* Track locals eagerly and only capture globals that the scope touches.
  Whenever a global is first accessed or mutated within a scope we keep
  recording it until the scope ends, and we ignore untouched globals so
  that module-wide dumps do not occur. Skip builtins and imported modules
  even when they are referenced, and make sure this choice is clearly
  documented. The concrete mechanics of
  detecting these accesses will be documented in a follow-up issue.

* Once a global becomes tracked, emit its value on every subsequent step,
  just like we do for locals.

* Do not apply name-based filtering for now. Even dunder variables and
  function objects should appear in the trace so we can evaluate the
  output. Prepare clear `/examples` programs that demonstrate the
  unfiltered captures for product review.

* Capture locals for non-function scopes such as class bodies,
  comprehensions, and generator expressions so their state appears in
  the trace.

* Keep using the existing `encode_value` implementation. Only extend it
  when the recorder would otherwise crash; sampling improvements arrive
  with the dedicated encoder work.

### Further research
We can improve our idea how to implement the issue by looking at the following:
- Check ../codetracer-ruby-recorder which also tries to record the values but for Ruby. Maybe we can use some ideas from there.
- Check ../runtime_tracing to understand what capabilities the tracing library supports.
- Draft a separate issue describing the instrumentation we need for
  tracking first-time global accesses within a scope.

### Status
High priority - not started.
