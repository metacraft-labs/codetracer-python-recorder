# File-Level SRP Plan (TL;DR)

## Goal
Give every Rust and Python file one clear job without breaking public APIs.

## Where we are messy
- `src/lib.rs` mixes module wiring, logging, and session control.
- `src/runtime_tracer.rs` glues together activation logic, writers, and value encoding.
- `src/tracer.rs` holds the trait, sys.monitoring glue, and storage helpers in one blob.
- `codetracer_python_recorder/api.py` blends auto-start side effects with the public API.

## Target layout
### Rust
| Topic | File |
| --- | --- |
| PyO3 entry + re-exports | `src/lib.rs` |
| Logging defaults | `src/logging.rs` |
| Session lifecycle (`start/stop/is_tracing`) | `src/session.rs` |
| Runtime façade | `src/runtime/mod.rs` |
| Activation toggles | `src/runtime/activation.rs` |
| Value encoding | `src/runtime/value_encoder.rs` |
| Trace file paths + writer setup | `src/runtime/output_paths.rs` |
| Monitoring shared types | `src/monitoring/mod.rs` |
| Tracer trait + dispatcher | `src/monitoring/tracer.rs` |
| Code caching | `src/code_object.rs` |

### Python
| Topic | File |
| --- | --- |
| Public API (`start`, `stop`, constants) | `codetracer_python_recorder/api.py` |
| Session handle | `codetracer_python_recorder/session.py` |
| Auto-start logic | `codetracer_python_recorder/auto_start.py` |
| Format helpers | `codetracer_python_recorder/formats.py` |
| Package exports | `codetracer_python_recorder/__init__.py` |

## Order of attack
1. **Stabilise baseline** – run `just test`, capture a sample trace.
2. **Split core Rust files**
   - Extract logging + session first so imports settle.
3. **Break up runtime tracer**
   - Activation, value encoding, and output paths can happen in parallel once the new module exists.
4. **Split monitoring helpers**
   - Move shared types into `monitoring/mod.rs` and keep the trait + dispatcher in `monitoring/tracer.rs`.
5. **Restructure Python package**
   - Create the helper modules, keep API signatures the same, update imports.
6. **Clean up**
   - Delete stale comments, refresh docs, and re-run all tests.

## Proof of done
- No file carries unrelated responsibilities.
- Tests and trace fixtures match the pre-refactor behaviour.
- Reviewers can learn a subsystem by opening a single focused file.
