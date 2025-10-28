# Recorder Exit Code Policy – Status

## Relevant Design Docs
- `design-docs/adr/0017-recorder-exit-code-policy.md`
- `design-docs/recorder-exit-code-policy-implementation-plan.md`

## Workstream Progress
- ✅ **WS1 – Policy & Configuration Plumbing:** Added `propagate_script_exit` across `RecorderPolicy`, `PolicyUpdate`, PyO3 bindings, env parsing, and Python helpers; introduced `CODETRACER_PROPAGATE_SCRIPT_EXIT`; updated Rust + Python unit coverage; rebuilt the dev wheel (`maturin develop --features integration-test`) and verified via `just test`.
- ✅ **WS2 – CLI Behaviour & Warning Surface:** CLI now defaults to returning `0` on successful recordings, exposes `--propagate-script-exit`/`--no-propagate-script-exit`, emits a warning when suppressing non-zero script exits, and preserves non-zero statuses for recorder failures; added regression coverage (`test_exit_payloads.py`) and executed `just dev test`.
- ✅ **WS3 – Documentation, Tooling, and Release Notes:** README and onboarding docs describe the new default, flags, and env overrides; changelog notes the behavioural change; `just dev test` re-run after docs to confirm tooling.

## Current Focus
- Monitor downstream consumers for feedback on the new default and capture any follow-up doc gaps or migration notes.
