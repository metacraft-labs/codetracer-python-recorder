from __future__ import annotations

import json
from pathlib import Path
from typing import Iterable

import pytest

native = pytest.importorskip("codetracer_python_recorder.codetracer_python_recorder")

if not hasattr(native, "encode_value_fixture"):
    pytest.skip(
        "encode_value_fixture helper missing; build with integration-test feature",
        allow_module_level=True,
    )


FIXTURE_DIR = Path(__file__).resolve().parents[2] / "data" / "values"


def _load_cases() -> Iterable[tuple[str, dict[str, object]]]:
    cases: list[tuple[str, dict[str, object]]] = []
    for path in sorted(FIXTURE_DIR.glob("*.json")):
        contents = json.loads(path.read_text(encoding="utf-8"))
        for case in contents.get("cases", []):
            cases.append((path.name, case))
    return cases


@pytest.mark.parametrize(("fixture_name", "case"), _load_cases())
def test_value_encoding_contract(fixture_name: str, case: dict[str, object]) -> None:
    encoded = native.encode_value_fixture(case["code"], case["expr"])
    actual = json.loads(encoded)
    assert actual == case["expected"], f"{fixture_name}::{case['name']}"
