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

### Proposed solutions

- Check ../codetracer-ruby-recorder which also tries to record the values but for Ruby. Maybe we can use some ideas from there.
- Check ../runtime_tracing to understand what capabilities the tracing library supports.
- Maybe we can listen on the INSTRUCTION event and track opcodes which change the local/global state (basically all STORE_* and DELETE_* opcodes). Here's a pure Python sketch that a friend gave me (I don't trust him though.) We might use ideas from it to implement our Rust recorder:
```py
import dis, sys

MUTATING = {
    "STORE_FAST", "DELETE_FAST",
    "STORE_NAME", "DELETE_NAME",
    "STORE_GLOBAL", "DELETE_GLOBAL",
    "STORE_DEREF", "DELETE_DEREF",
}

index = {}  # code -> {offset: (opname, name)}

def index_code(code):
    d = {}
    for ins in dis.get_instructions(code):
        if ins.opname in MUTATING:
            d[ins.offset] = (ins.opname, ins.argval)  # name bound/deleted
    index[code] = d

TOOL = sys.monitoring.DEBUGGER_ID
sys.monitoring.use_tool_id(TOOL, "varspy")

def on_instr(code, offset):
    d = index.get(code)
    info = d and d.get(offset)
    if not info:
        return sys.monitoring.DISABLE  # silence this location forever
    # ... here: grab frame (e.g. via PyEval_GetFrame in PyO3) and read f_locals/f_globals ...
    # opname, name = info
    # value = ...
    # emit change event

sys.monitoring.register_callback(TOOL, sys.monitoring.events.INSTRUCTION, on_instr)
sys.monitoring.set_events(TOOL, sys.monitoring.events.INSTRUCTION)
```

Right now it's not clear in detail how to implement the feature. When more becomes clear we have to add all details to the issue description.

### Status
High priority - not started.
