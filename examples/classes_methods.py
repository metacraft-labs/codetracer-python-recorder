"""Example: classes, instance/class/static methods, and a property."""


class Counter:
    def __init__(self, start: int = 0) -> None:
        self._n = start

    def inc(self, by: int = 1) -> int:
        self._n += by
        return self._n

    @property
    def double(self) -> int:
        return self._n * 2

    @classmethod
    def start_at(cls, n: int) -> "Counter":
        return cls(n)

    @staticmethod
    def add(a: int, b: int) -> int:
        return a + b


def main() -> None:
    c = Counter.start_at(2)
    x = c.inc()
    y = Counter.add(3, 4)
    print("ok", x, y, c.double)


if __name__ == "__main__":
    main()

