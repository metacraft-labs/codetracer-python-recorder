"""Render a concise Rust coverage summary table from cargo-llvm-cov JSON."""

from __future__ import annotations

import argparse
import json
import pathlib
import sys
from typing import Iterable, List, Tuple


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
    try:
        payload = json.loads(summary_path.read_text(encoding="utf-8"))
    except FileNotFoundError as exc:
        raise SystemExit(f"Rust coverage summary not found: {summary_path}") from exc

    repo_root = repo_root.resolve()
    rows: List[Tuple[str, int, int, float]] = []

    for dataset in payload.get("data", []):
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

    rows.sort(key=lambda item: item[0])
    return rows


def render(rows: List[Tuple[str, int, int, float]]) -> str:
    if not rows:
        return "Rust coverage summary: no project files found"

    name_width = max(len(name) for name, *_ in rows)
    lines = ["Rust coverage summary (lines):", f"{'Name'.ljust(name_width)}  Lines  Miss  Cover"]

    for name, total, missed, percent in rows:
        lines.append(f"{name.ljust(name_width)}  {total:5d}  {missed:4d}  {percent:5.1f}%")

    return "\n".join(lines)


def main(argv: Iterable[str] | None = None) -> int:
    args = parse_args(argv)
    rows = load_rows(args.summary_path, args.root)
    output = render(rows)
    print(output)
    return 0


if __name__ == "__main__":
    sys.exit(main())
