"""Example: simple multithreading with join."""

from __future__ import annotations

import threading


def worker(name: str, out: list[str]) -> None:
    out.append(name)


def main() -> None:
    out: list[str] = []
    t1 = threading.Thread(target=worker, args=("t1", out))
    t2 = threading.Thread(target=worker, args=("t2", out))
    t1.start(); t2.start()
    t1.join(); t2.join()
    print("ok", sorted(out))


if __name__ == "__main__":
    main()

