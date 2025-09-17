"""Example: generator, async function, and async generator."""

from __future__ import annotations

import asyncio


def gen(n: int):
    for i in range(n):
        yield i * i


async def async_add(a: int, b: int) -> int:
    return a + b


async def agen(n: int):
    for i in range(n):
        yield i + 1


def main() -> None:
    s = sum(gen(5))
    total = asyncio.run(async_add(3, 4))

    async def consume() -> int:
        acc = 0
        async for x in agen(3):
            acc += x
        return acc

    acc = asyncio.run(consume())
    print("ok", s, total, acc)


if __name__ == "__main__":
    main()

