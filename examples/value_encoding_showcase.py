"""Showcase script for the expanded value encoding surface area.

Run this example with the recorder CLI to inspect the emitted trace:

    python -m codetracer_python_recorder --format=json @examples/value_encoding_showcase.py

The script builds representative values spanning the encoder workstreams:
scalars/numerics, text & binary payloads, container guardrails, structured &
temporal objects, and repr-based fallback paths.
"""

from __future__ import annotations

import collections
import dataclasses
import datetime
import decimal
import fractions
import math
import pathlib
from enum import Enum
from types import SimpleNamespace
from typing import NamedTuple


decimal.getcontext().prec = 28


@dataclasses.dataclass
class Settings:
    environment: str
    retries: int
    tags: list[str]


class Point(NamedTuple):
    x: float
    y: float


class Color(Enum):
    RED = 1
    GREEN = 2
    BLUE = 3


class UnhandledCustom:
    """No dedicated encoder; exercises repr fallback."""


class ExplodingRepr:
    """Raises from __repr__ to surface repr_error telemetry."""

    def __repr__(self) -> str:  # pragma: no cover - manual demo utility
        raise RuntimeError("repr failed intentionally")


class VerboseRepr:
    """Produces a long repr string to trigger truncation metadata."""

    def __init__(self, width: int) -> None:
        self.width = width

    def __repr__(self) -> str:
        payload = "x" * self.width
        return f"VerboseRepr<{payload}>"


class PathLike:
    """Implements the path protocol without being a pathlib object."""

    def __init__(self, value: str) -> None:
        self._value = value

    def __fspath__(self) -> str:
        return self._value


def scalars_and_numerics() -> dict[str, object]:
    bool_val = True
    int_small = 42
    int_big_positive = 2**80
    int_big_negative = -(2**100)
    #float_nan = float("nan")
    #float_infinity = float("inf")
    float_neg_zero = math.copysign(0.0, -1.0)
    complex_number = complex(-2.5, 0.125)
    decimal_value = decimal.Decimal("12345.6789012345678901234567")
    fraction_value = fractions.Fraction(355, 113)
    return locals()


def text_binary_and_paths() -> dict[str, object]:
    short_text = "Codetracer keeps previews short."
    long_text = "Block " + "|".join(str(i).zfill(3) for i in range(300))
    bytes_payload = bytes(range(256)) * 4
    bytearray_payload = bytearray(range(128)) * 3
    memoryview_payload = memoryview(bytes_payload)
    filesystem_path = pathlib.Path(__file__).resolve()
    pure_posix = pathlib.PurePosixPath("/opt/codetracer/data")
    custom_pathlike = PathLike("/tmp/codetracer-preview.txt")
    return locals()


def collections_and_recursion() -> dict[str, object]:
    set_preview = {1, 2, 3, 4}
    frozenset_preview = frozenset({"a", "b", "c"})
    range_preview = range(-5, 20, 3)
    deque_preview = collections.deque(range(10), maxlen=5)

    shared_leaf = {"id": 1}
    repeated_reference = [shared_leaf, shared_leaf]

    recursive_list: list[object] = [0, 1]
    #recursive_list.append(recursive_list)

    nested_mapping = {
        "list_with_cycle": recursive_list,
        "shared": repeated_reference,
        "dict": {"key": "value", "alias": shared_leaf},
    }

    large_tuple_slice = tuple(range(64))
    breadth_limited_sequence = [tuple(range(10)) for _ in range(50)]

    return locals()


def structured_and_temporal() -> dict[str, object]:
    settings = Settings(environment="prod", retries=3, tags=["billing", "payments"])
    point = Point(1.25, -3.5)
    color = Color.GREEN
    namespace = SimpleNamespace(alpha=1, beta="two", gamma=[3, 4, 5])

    tz_west = datetime.timezone(datetime.timedelta(hours=-7), name="Pacific")
    tz_east = datetime.timezone(datetime.timedelta(hours=2), name="CEST")

    timestamp = datetime.datetime(
        2024, 6, 1, 12, 34, 56, 789012, tzinfo=datetime.timezone.utc
    )
    local_date = datetime.date(2025, 1, 15)
    wall_clock = datetime.time(6, 45, 3, 21000, tzinfo=tz_east)
    elapsed = datetime.timedelta(days=5, seconds=42, microseconds=100)
    attached_timezone = tz_west

    return locals()


def fallback_paths() -> dict[str, object]:
    unhandled = UnhandledCustom()
    repr_error = ExplodingRepr()
    truncated_repr = VerboseRepr(width=600)
    # Depth guard: nest lists deeply to encourage a depth-based slice.
    depth_guard = current = []
    for _ in range(40):
        new_level: list[object] = []
        current.append(new_level)
        current = new_level

    return locals()


def main() -> dict[str, dict[str, object]]:
    showcase = {
        "scalars_and_numerics": scalars_and_numerics(),
        "text_binary_and_paths": text_binary_and_paths(),
        "collections_and_recursion": collections_and_recursion(),
        "structured_and_temporal": structured_and_temporal(),
        "fallback_paths": fallback_paths(),
    }
    return showcase


if __name__ == "__main__":  # pragma: no cover - example entry point
    main()
