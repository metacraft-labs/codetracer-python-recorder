Examples for exercising the Rust‑backed recorder during development.

Run any script via the module CLI so tracing is consistently enabled:

  python -m codetracer_python_recorder --format=json examples/<script>.py

Scripts

- basic_args.py: Demonstrates positional‑only, pos‑or‑kw, kw‑only, *args, **kwargs.
- exceptions.py: Raises, catches, and prints an exception in except.
- classes_methods.py: Instance, @classmethod, @staticmethod, and a property.
- recursion.py: Direct recursion (factorial) and mutual recursion.
- generators_async.py: A generator, async function, and async generator.
- context_and_closures.py: A context manager and a nested closure.
- threading.py: Two threads invoking traced functions and joining.
- imports_side_effects.py: Module‑level side effects vs main guard.
- kwargs_nested.py: Nested kwargs structure to validate structured encoding.

All scripts are deterministic and print minimal output.
