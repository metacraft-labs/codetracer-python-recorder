"""Demonstrate locals snapshots while a generator resumes and yields values."""

def produce() -> None:
    acc = 0
    for item in range(3):
        acc += item
        note = f"step={item}, acc={acc}"
        yield item, acc, note


def main() -> None:
    g = produce()
    print(next(g))
    print(next(g))


if __name__ == "__main__":
    main()
