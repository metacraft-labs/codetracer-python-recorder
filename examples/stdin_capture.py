"""Exercise stdin capture with sequential reads."""
from __future__ import annotations

import sys


def _describe(label: str, value: str | None) -> None:
    if value is None:
        print(f"{label}: <EOF>")
    elif value == "":
        print(f"{label}: <empty string>")
    else:
        print(f"{label}: {value!r}")


def main() -> None:
    if sys.stdin.isatty():
        print("stdin is attached to a TTY; pipe data to exercise capture.")
        print(
            "Example: printf 'first\\nsecond\\nthird' | "
            "python -m codetracer_python_recorder examples/stdin_capture.py"
        )
        return

    print("Inspecting stdin via input()/readline()/read()")

    try:
        first = input()
    except EOFError:
        first = None
    _describe("input()", first)

    second = sys.stdin.readline()
    _describe("sys.stdin.readline()", second if second else None)

    remaining = sys.stdin.read()
    _describe("sys.stdin.read()", remaining if remaining else None)

    print("Done reading from stdin.")


if __name__ == "__main__":
    main()
