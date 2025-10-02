"""Render a concise Rust coverage summary table from cargo-llvm-cov JSON."""

from __future__ import annotations

import argparse
import json
import pathlib
import sys
from typing import Dict, Iterable, List, Tuple


def _load_payload(summary_path: pathlib.Path) -> Dict:
    try:
        return json.loads(summary_path.read_text(encoding="utf-8"))
    except FileNotFoundError as exc:
        raise SystemExit(f"Rust coverage summary not found: {summary_path}") from exc


def parse_args(argv: Iterable[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__ or "")
    parser.add_argument(
        "summary_path",
        type=pathlib.Path,
        help="Path to cargo-llvm-cov JSON summary (e.g. summary.json)",
    )
    parser.add_argument(
        "--root",
        type=pathlib.Path,
        default=pathlib.Path.cwd(),
        help="Repository root used to relativise file paths (default: current working directory)",
    )
    return parser.parse_args(argv)


def load_rows(summary_path: pathlib.Path, repo_root: pathlib.Path) -> List[Tuple[str, int, int, float]]:
    payload = _load_payload(summary_path)
    rows, _, _ = load_summary(summary_path, repo_root, payload)
    return rows


def load_summary(
    summary_path: pathlib.Path,
    repo_root: pathlib.Path,
    payload: Dict | None = None,
) -> Tuple[List[Tuple[str, int, int, float]], Dict[str, float], Dict[str, Tuple[int, int]]]:
    if payload is None:
        payload = _load_payload(summary_path)

    repo_root = repo_root.resolve()
    rows: List[Tuple[str, int, int, float]] = []

    totals: Dict[str, float] = {}
    crate_totals: Dict[str, Tuple[int, int]] = {}

    for dataset in payload.get("data", []):
        dataset_totals = dataset.get("totals", {})
        if dataset_totals:
            totals = dataset_totals.get("lines", dataset_totals)
        for entry in dataset.get("files", []):
            filename = entry.get("filename")
            if not filename:
                continue
            path = pathlib.Path(filename)
            try:
                rel_path = path.resolve().relative_to(repo_root)
            except Exception:
                # Skip entries outside the repository (stdlib, third-party deps, etc.).
                continue

            line_summary = (entry.get("summary") or {}).get("lines") or {}
            total = int(line_summary.get("count", 0))
            covered = int(line_summary.get("covered", 0))
            missed = max(total - covered, 0)
            percent = float(line_summary.get("percent", 0.0))
            rows.append((rel_path.as_posix(), total, missed, percent))

            crate_key = _crate_key(rel_path)
            agg_total, agg_covered = crate_totals.get(crate_key, (0, 0))
            crate_totals[crate_key] = (agg_total + total, agg_covered + covered)

    rows.sort(key=lambda item: item[0])
    return rows, totals, crate_totals


def _crate_key(rel_path: pathlib.Path) -> str:
    parts = rel_path.parts
    if len(parts) >= 3 and parts[1] == "crates":
        return "/".join(parts[:3])
    return parts[0]


def render(rows: List[Tuple[str, int, int, float]]) -> str:
    if not rows:
        return "Rust coverage summary: no project files found"

    name_width = max(len(name) for name, *_ in rows)
    lines = ["Rust coverage summary (lines):", f"{'Name'.ljust(name_width)}  Lines  Miss  Cover"]

    for name, total, missed, percent in rows:
        lines.append(f"{name.ljust(name_width)}  {total:5d}  {missed:4d}  {percent:5.1f}%")

    return "\n".join(lines)


def render_crates(crate_totals: Dict[str, Tuple[int, int]]) -> str:
    if not crate_totals:
        return "Rust crate coverage summary: no project files found"

    name_width = max(len(name) for name in crate_totals)
    lines = [
        "Rust coverage summary by crate (lines):",
        f"{'Crate'.ljust(name_width)}  Lines  Miss  Cover",
    ]

    for name in sorted(crate_totals):
        total, covered = crate_totals[name]
        missed = max(total - covered, 0)
        percent = 0.0 if total == 0 else (covered / total) * 100
        lines.append(f"{name.ljust(name_width)}  {total:5d}  {missed:4d}  {percent:5.1f}%")

    return "\n".join(lines)


def main(argv: Iterable[str] | None = None) -> int:
    args = parse_args(argv)
    rows, _, crate_totals = load_summary(args.summary_path, args.root)
    crate_output = render_crates(crate_totals)
    file_output = render(rows)
    print(crate_output)
    print()
    print(file_output)
    return 0


if __name__ == "__main__":
    sys.exit(main())
