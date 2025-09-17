# Questions from 2025-09-17

- Q: For ISSUE-012, when we say "all variables accessible at the
  step", should we capture module-level globals via `frame.f_globals`
  on every line, or limit ourselves to the executing frame's locals?
  If globals are in scope, do we restrict them to files under the
  traced program's root to avoid enormous dumps?
  
  A: It's good to avoid large dumps. Let's only record those globals
  which are actually accessed or modified in the given scope. So at
  each step in a given scope we track if a new global was accessed,
  and if so we track it until the end of the scope. We need to
  describe how to implement this in practice in a separate issue.

- Q: Do we have a product requirement for filtering internal names
  when capturing variable state for ISSUE-012? The pure-Python tracer
  currently skips double-underscore names and function objects—should
  both recorders follow that rule, or do we need an explicit
  allow/deny policy (e.g., keep `__name__`, drop modules)?
  
  A: Let's not filter internal names initially. We might later change
  after we see what the first implementation looks like. It's good to
  prepare good example scripts in the `/examples` folder which will
  allow me to see what a typical trace will look like when we don't
  filter anything.

- Q: Before the dedicated value-encoder work lands, how much sampling
  should we apply to large containers while implementing ISSUE-012? Do
  we need hard caps on length/depth today, and if so what limits keep
  the UI usable until the encoder revamp?

  A: Just use the `encode_value` function as is and don't change it
  unless we have missing functionality which causes our recorder to
  crash. We'll work on this later.

# Questions from 2025-09-17 - iteration 2

- Q: For ISSUE-012, when a frame resolves a global name to a builtin
  or imported module (e.g., calling `len` or referencing `math`), do
  we record that object once it is touched? Capturing builtins/modules
  might add noise, but skipping them could hide state the UI needs.

  A: Don't capture them. Clearly document this design choice

- Q: Once a global name becomes tracked within a scope, should we
  re-emit its value on every subsequent step like we do for locals, or
  only when the value changes? The spec says "keep recording it until
  the scope ends" but does not define the cadence.
  
  A: We should treat is in the same way as locals - emit the value at each step.

- Q: How should we treat non-function scopes such as class bodies,
  comprehensions, and generator expressions? Do we capture their
  locals (e.g., class attributes or comprehension temporaries) at each
  line, or restrict ISSUE-012 to function/module frames?
  
  A: Let's capture their locals as well.
# Questions from 2025-09-17 - iteration 3

- Q: For ISSUE-010, do we want both recorders to apply the structured
  dict encoding only at kwargs capture sites, or should ordinary dict
  values across all contexts keep using the (key, value) sequence
  encoding? We need a decision so we can align specs, fixtures, and
  schema docs.
  
  A: All dicts will use the (key, value) sequence encoding. 

- Q: ISSUE-012 requires us to start recording globals only after a
  scope touches them, but we don't yet have the follow-up
  instrumentation spec for detecting those touches. Should we invest
  now in subscribing to instruction-level events (e.g., tracking
  LOAD_GLOBAL/STORE_GLOBAL) to meet this requirement, or is it
  acceptable to ship an initial version that records only locals while
  we wait for the dedicated instrumentation plan?
  
  A: Ship an initial version which only records locals. Create a
  separate issue for the instrumentation part.

- Q: ISSUE-012 touches both the pure-Python and Rust-backed
  recorders. Can we stage the delivery (ship the pure-Python recorder
  capturing locals/globals first, then port the same behavior to the
  Rust tracer), or do you need both recorders to gain parity in the
  same release?
  
  A: ISSUE-012 concerns only the Rust-based recorder. The pure-Python
  recorder is deprecated and will not be developed in the future. We
  should document this clearly in the repo docs.
# Questions from 2025-09-17 - iteration 4

- Q: ISSUE-012: When generators or coroutines yield control back to
  the caller, do we need to emit a full locals snapshot on the
  `PY_YIELD` event (and again on the next `PY_RESUME`) so the UI can
  show state at suspension points, or is capturing locals on `LINE`
  events alone sufficient?
  
  A: We don't need this for now. But add scripts relevant to this
  quesition in `/examples` in order to be able to see how the UI
  behaves

- Q: ISSUE-010: Since all dicts will use the (key, value) sequence
  encoding, what should the recorder do when it encounters a
  non-string key? Should we raise and terminate tracing to stay
  fail-fast, or keep the current best-effort fallback that encodes the
  key via `repr`?
  
  A: Just encode the key using `encode_value`. We don't care if it is
  a string or not.

- Q: ISSUE-009: Now that the pure-Python recorder is deprecated, do we
  still need to rename its list `lang_type` to `List` for parity, or
  can we leave it untouched and focus on bringing the Rust recorder in
  line with the spec?
  
  A: We don't need to rename it.

# Questions from 2025-09-17 - iteration 5

- Q: ISSUE-012: `sys.monitoring` fires `LINE` events for module-level
  execution where `frame.f_locals` mirrors
  `frame.f_globals`. Capturing those frames today would dump the
  entire module namespace on every step, while ISSUE-013 (global
  tracking) is still pending. Should we skip module-level frames until
  the global-access instrumentation lands, or do you prefer we emit
  those snapshots even though they will include untouched globals?
  
  A: We should emit those snapshots, because some scripts run
  important code at the global scope. 

- Q: Public API (`codetracer.start`): right now the Rust backend
  silently falls back to binary output when the `format` argument is
  unrecognized. Do you want us to keep that permissive fallback, or
  should we raise an error so callers immediately know the format
  value is unsupported?
  
  A: We should raise an error

# Questions from 2025-09-18 - iteration 6

- Q: ISSUE-012: Module-level LINE events will capture locals dict
  entries for imported modules (e.g., math) and __builtins__. Should
  we include those objects in the emitted locals snapshots, or skip
  them to honor the earlier guidance to avoid recording
  modules/builtins when they surface via globals?

  A: It depends on how easy it is. If we can easily filter imported
  modules and __builtins__ then let's do it. If we require more
  instrumentation (e.g. hooking to INSTRUCTION events) then let's not.
