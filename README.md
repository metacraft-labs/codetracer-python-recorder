## CodeTracer Recorders (Monorepo)

This repository now hosts two related projects:

- codetracer-pure-python-recorder — the original pure-Python prototype built on `sys.settrace`. **This recorder is now deprecated** and kept only for archival purposes.
- codetracer-python-recorder — the Rust-backed Python extension module (PyO3) that supersedes the pure-Python tracer. It is the source of truth for new tracing behaviour.

> [!WARNING]
> Both projects are early-stage prototypes. Contributions and discussion are welcome!

### codetracer-pure-python-recorder *(deprecated)*

Install from PyPI:

```bash
pip install codetracer-pure-python-recorder
```

CLI usage:

```bash
codetracer-record <path to python file>
# produces several trace json files in the current directory
# or in the folder of `$CODETRACER_DB_TRACE_PATH` if such an env var is defined
```

During development you can also run it directly:

```bash
python src/trace.py <path to python file>
# produces several trace json files in the current directory
# or in the folder of `$CODETRACER_DB_TRACE_PATH` if such an env var is defined
```

### codetracer-python-recorder (Rust-backed)

The actively developed recorder is implemented in Rust with PyO3 and lives under `codetracer-python-recorder/`.

Locals capture behaviour (introduced in ISSUE-012):

- Every `LINE` event recorded through `sys.monitoring` now emits the full set of locals visible to that frame. This applies to functions, class bodies, comprehensions, generators/coroutines, and module-level frames.
- Snapshots are emitted on **every** step; values are never diffed or elided. The recorded state reflects the locals **at the start of the reported line**.
- Imported modules (`types.ModuleType`) and `__builtins__` are filtered from snapshots when present, keeping the trace focused on user data.
- Global variables are not tracked yet; that work will land with ISSUE-013. Until then, module-level locals mirror the global namespace.

Basic workflow:

- Build/dev install the Rust module:
  - `maturin develop -m codetracer-python-recorder/Cargo.toml`
- Use in Python:
  - `from codetracer_python_recorder import trace`
  - `python -m codetracer_python_recorder --codetracer-format=json examples/locals_snapshot.py`

### Future directions

The current Python support is an unfinished prototype. We can finish it. In the future, it may be expanded to function in a way to similar to the more complete implementations, e.g. [Noir](https://github.com/blocksense-network/noir/tree/blocksense/tooling/tracer).

Currently it's very similar to our [Ruby tracer](https://github.com/metacraft-labs/ct-ruby-tracer)

#### Current approach: sys.settrace API

Currently we're using the sys.settrace API: https://docs.python.org/3/library/sys.html#sys.settrace .
This is very flexible and can function with probably multiple Python versions out of the box. 
However, this is limited:

* it's not optimal
* it can't track more detailed info/state, needed for some CodeTracer features(or for more optimal replays).

For other languages, we've used a more deeply integrated approach: patching the interpreter or VM itself (e.g. Noir).

#### Patching the VM

This can be a good approach for Python as well: it can let us record more precisely subvalues, assignments and subexpressions and to let
some CodeTracer features work in a deeper/better way.

One usually needs to add additional logic to places where new opcodes/lines are being ran, and to call entries/exits. Additionally
tracking assignments can be a great addition, but it really depends on the interpreter internals.

#### Filtering

It would be useful to have a way to record in detail only certain periods of the program, or certain functions or modules: 
we plan on expanding the [trace format](https://github.com/metacraft-labs/runtime_tracing/) and CodeTracer' support, so that this is possible. It would let one be able to record interesting
parts of even long-running or more heavy programs.

### Contributing

We'd be very happy if the community finds this useful, and if anyone wants to:

* Use and test the Python support or CodeTracer.
* Provide feedback and discuss alternative implementation ideas: in the issue tracker, or in our [discord](https://discord.gg/qSDCAFMP).
* Contribute code to enhance the Python support of CodeTracer.
* Provide [sponsorship](https://opencollective.com/codetracer), so we can hire dedicated full-time maintainers for this project.

### Legal info

LICENSE: MIT

Copyright (c) 2025 Metacraft Labs Ltd
