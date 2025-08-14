"""CLI entrypoint for the pure-Python recorder (namespaced).

This defers to the existing trace.main implementation to preserve behavior
and compatibility with the current tests and CLI usage.
"""

from typing import List, Optional


def main(argv: Optional[List[str]] = None) -> None:
    from trace import main as trace_main

    return trace_main(argv)
