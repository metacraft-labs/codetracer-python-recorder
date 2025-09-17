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

# Questions from 2025-09-17 (later)

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
