# Coverage Plan (Simple View)

## Goals
- Produce Rust + Python coverage reports without slowing developers down.
- Keep outputs in `target/coverage` so everyone can inspect them later.
- Make it easy to add CI enforcement once numbers stabilise.

## Tools
- Rust: `cargo llvm-cov` (works with `nextest`).
- Python: `pytest --cov` limited to `codetracer_python_recorder`.

## Local commands
- `just coverage-rust` → creates `target/coverage/rust/lcov.info` and a summary JSON/HTML when needed.
- `just coverage-python` → runs pytest with XML + JSON reports under `target/coverage/python/`.
- `just coverage` → runs both steps.

## CI plan (phase 1)
- Add optional jobs on Ubuntu + Python 3.12 that reuse the Just commands.
- Upload artefacts (Rust lcov + summary, Python XML/JSON).
- Mark jobs `continue-on-error` until results settle.
- Post a PR comment summarising the numbers from the JSON reports.

## Rollout steps
1. Land the Just targets and document them in `tests/README.md`.
2. Wire up the optional CI jobs.
3. Watch runtimes + artefact sizes for a few runs.
4. When stable, drop `continue-on-error` and discuss minimum coverage thresholds.

## Risks + mitigations
- **Slow runs** → limit coverage to one matrix entry, reuse caches.
- **Large artefacts** → compress HTML or keep only lcov/XML.
- **PyO3 quirks** → run `cargo llvm-cov` with the same feature flags as normal tests.
