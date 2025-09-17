# General Issues

# Issues Breaking Declared Relations

This document lists concrete mismatches that cause the relations in `relations.md` to fail.

It should be structured like so:
```md
## REL-001
### ISSUE-001-001
#### Description
Blah blah blah
#### Proposed solution
Blah blah bleh

### ISSUE-001-002
...

## REL-002
...
```

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

