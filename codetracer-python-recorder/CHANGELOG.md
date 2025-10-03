# codetracer-python-recorder — Change Log

## Unreleased
- Documented the error-handling policy. README now lists the `RecorderError` hierarchy, policy hooks (`configure_policy`, JSON trailers), exit codes, and sample handlers so Python callers can consume structured failures.
- Added an onboarding guide under `docs/onboarding/error-handling.md` with migration steps for downstream tools.
- Recorded assertion guidance for contributors: prefer `bug!`/`ensure_internal!` over raw `panic!`/`.unwrap()` and keep `debug_assert!` paired with classified errors.
