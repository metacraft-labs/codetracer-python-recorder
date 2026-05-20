"""codetracer_pure_python_recorder — pure-Python *reference* recorder.

Namespaced package wrapper for the pure-Python implementation of the
CodeTracer Python recorder. The recorder logic itself lives in the
sibling top-level ``trace`` module; this package mainly provides a
console-script entry point (``cli.py``) so the recorder can be invoked
as ``codetracer-record``.

The pure-Python recorder is **JSON-only by design** and serves as the
cross-validation oracle for the production native recorder at
``../codetracer-python-recorder/`` (Rust + PyO3, CTFS v3 output). The
test suite runs both recorders against the same programs and uses
``ct print --json-events`` to bring the native recorder's CTFS output
back into a comparable JSON shape — see this package's ``README.md``
and the sibling project's ``tests/python/test_cli_integration.py`` for
the full rationale.

Do not migrate this package to CTFS without coordinating with the
test framework.
"""
__all__ = []
