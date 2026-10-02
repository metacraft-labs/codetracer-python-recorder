"""codetracer_pure_python_recorder — pure-Python *reference* recorder.

Namespaced package wrapper for the pure-Python implementation of the
CodeTracer Python recorder. The recorder logic itself lives in the
sibling top-level ``trace`` module; this package mainly provides a
console-script entry point (``cli.py``) so the recorder can be invoked
as ``codetracer-record``.

This recorder is a **test oracle, not a production recorder**. It
writes JSON only, and CodeTracer cannot open that output: it is not a
recording. The production recorder is ``../codetracer-python-recorder/``
(Rust + PyO3, CTFS ``.ct`` output). The test suite records the same
programs with both, converts the ``.ct`` with ``ct print``, and compares
-- see ``codetracer-python-recorder/tests/python/test_pure_oracle.py``
and this package's ``README.md`` / ``AGENTS.md``.

Do not migrate this package to CTFS: that would remove the independent
oracle.
"""
__all__ = []
