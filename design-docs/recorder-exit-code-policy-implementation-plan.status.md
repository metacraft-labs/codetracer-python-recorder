# Recorder Exit Code Policy – Status

## Relevant Design Docs
- `design-docs/adr/0017-recorder-exit-code-policy.md`
- `design-docs/recorder-exit-code-policy-implementation-plan.md`

## Workstream Progress
- ✅ **WS1 – Policy & Configuration Plumbing:** Added `propagate_script_exit` across `RecorderPolicy`, `PolicyUpdate`, PyO3 bindings, env parsing, and Python helpers; introduced `CODETRACER_PROPAGATE_SCRIPT_EXIT`; updated Rust + Python unit coverage; rebuilt the dev wheel (`maturin develop --features integration-test`) and verified via `just test`.
- ☐ **WS2 – CLI Behaviour & Warning Surface:** _Not started._
- ☐ **WS3 – Documentation, Tooling, and Release Notes:** _Not started._

## Current Focus
- Confirm no downstream consumers rely on the legacy policy snapshot schema; prepare CLI changes for WS2.
