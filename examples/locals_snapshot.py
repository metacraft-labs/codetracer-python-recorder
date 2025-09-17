"""Showcase locals snapshots emitted on every line for ordinary functions."""

def bump(n: int) -> int:
    total = n
    total += 1
    shadow = total * 2
    return total + shadow


def main() -> None:
    counter = 10
    result = bump(counter)
    result += 5
    print(f"counter={counter}, result={result}")


if __name__ == "__main__":
    main()
