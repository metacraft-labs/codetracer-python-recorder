"""Example: context manager and nested closures."""

from contextlib import contextmanager


@contextmanager
def tag(name: str):
    # Minimal context; no side effects beyond yielding
    yield f"<{name}>"


def make_multiplier(factor: int):
    def mul(x: int) -> int:
        return x * factor

    return mul


def main() -> None:
    with tag("x") as t:
        mul3 = make_multiplier(3)
        print("ok", t, mul3(5))


if __name__ == "__main__":
    main()

