# Function-Level SRP Status Snapshot

## Completed
- ✅ Baseline captured: `just test` passes and JSON/Binary fixtures from `examples/value_capture_all.py` are stored for comparisons.
- ✅ Session bootstrap helpers in Rust and Python now handle directory checks, format resolution, and activation path cleanup.
- ✅ `frame_inspector` + beefed-up `ActivationController` keep frame grabbing and gating logic out of the callbacks.
- ✅ `value_capture` helpers own argument, scope, and return recording; callbacks just orchestrate.
- ✅ Logging is centralised and runtime tests cover return handling and activation teardown.
- ✅ Final cleanup pass removed TODOs and reran the full test recipe.

## Still open
- Write the short explainer on activation gating/frame search if reviewers still need it.
- Decide whether to snapshot fresh fixtures post-refactor.
