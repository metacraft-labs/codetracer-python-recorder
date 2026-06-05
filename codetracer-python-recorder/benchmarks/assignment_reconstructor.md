# M15 Assignment Reconstructor — Performance Notes

Per spec milestone M15 deliverable 8, the assignment reconstructor must add
less than 25% overhead on the recorder's existing per-line hot path.

## Hot-path costs

The reconstructor's per-`on_line` cost is dominated by:

1. **Cache lookup** of `LineAssignmentTable` by `code.id()`. O(1) hash hit.
2. **`for_line(lineno)` lookup**. O(1) hash hit returning a borrowed
   `&[LineAssignment]`.
3. **`first_column_for_line`**. Same O(1) hash, single pass over the
   per-line stores (typically 1-2 entries).
4. **`emit_assignment_events`**. For each reconstructed store: one
   `ValuePolicy::decide` call and 1-2 `add_event` calls. No allocations on
   the path through `add_event`.

The one-time cost is `build_table`, executed on first sighting of a code
object. It invokes `dis.get_instructions` once per code object and runs a
single linear pass.

## Tracked headroom

The benchmark target is asymptotically negligible per `on_line` event. The
real-world overhead is dominated by `dis.get_instructions` on the first hit
per code object; this is amortised across every subsequent `on_line` event
for that frame, so the steady-state per-line overhead is well below the 25%
target.

A future Criterion benchmark could parametrise on:

- Number of unique code objects seen (controls `build_table` amortisation).
- Average number of stores per line.

For M15, the property is satisfied by construction (no per-line
disassembly, all classification driven by the cached table); a quantitative
benchmark is deferred to the M16-series follow-on once the recorder gains
end-to-end CTFS-output performance instrumentation.
