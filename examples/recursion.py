"""Example: direct and mutual recursion."""


def factorial(n: int) -> int:
    return 1 if n <= 1 else n * factorial(n - 1)


def is_even(n: int) -> bool:
    return n == 0 or is_odd(n - 1)


def is_odd(n: int) -> bool:
    return n != 0 and is_even(n - 1)


def main() -> None:
    print("ok", factorial(5), is_even(4), is_odd(7))


if __name__ == "__main__":
    main()

