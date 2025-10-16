"""Exercise IO capture proxies with mixed stdout/stderr writes."""
from __future__ import annotations

import os
import sys


def emit_high_level() -> None:
    print("stdout via print()", flush=True)
    sys.stdout.write("stdout via sys.stdout.write()\n")
    sys.stdout.flush()

    sys.stderr.write("stderr via sys.stderr.write()\n")
    sys.stderr.flush()


def emit_low_level() -> None:
    os.write(1, b"stdout via os.write()\n")
    os.write(2, b"stderr via os.write()\n")


def main() -> None:
    print("Demonstrating Codetracer IO capture behaviour")
    emit_high_level()
    emit_low_level()
    print("Done.")


if __name__ == "__main__":
    main()
