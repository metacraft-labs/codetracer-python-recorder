#!/usr/bin/env python3
"""Generate a Markdown coverage summary comment for CI."""

from __future__ import annotations

import argparse
import json
import pathlib
from typing import Dict, Iterable, List, Tuple

from render_rust_coverage_summary import load_summary as load_rust_summary

Row = Tuple[str, int, int, float]


def parse_args(argv: Iterable[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__ or "")
    parser.add_argument(
        "--rust-summary",
        type=pathlib.Path,
        required=True,
        help="Path to cargo-llvm-cov JSON summary",
    )
    parser.add_argument(
        "--python-json",
        type=pathlib.Path,
        required=True,
        help="Path to coverage.py JSON report",
    )
    parser.add_argument(
        "--output",
        type=pathlib.Path,
        required=True,
        help="Output Markdown file for the PR comment",
    )
    parser.add_argument(
        "--repo-root",
        type=pathlib.Path,
        default=pathlib.Path.cwd(),
        help="Repository root used to relativise file paths (default: current working directory)",
    )
    parser.add_argument(
        "--max-rows",
        type=int,
        default=20,
        help="Maximum number of per-file rows to display for each language (default: 20).",
    )
    return parser.parse_args(argv)


def _select_rows(rows: List[Row], max_rows: int) -> Tuple[List[Row], bool]:
    if max_rows <= 0 or len(rows) <= max_rows:
        return sorted(rows, key=lambda item: item[0]), False

    priority_sorted = sorted(rows, key=lambda item: (-item[2], item[0]))
    trimmed = priority_sorted[:max_rows]
    return sorted(trimmed, key=lambda item: item[0]), True


def _format_table(rows: List[Row], headers: Tuple[str, str, str, str]) -> List[str]:
    lines = [
        f"| {headers[0]} | {headers[1]} | {headers[2]} | {headers[3]} |",
        "| --- | ---: | ---: | ---: |",
    ]
    for name, total, missed, percent in rows:
        lines.append(
            f"| `{name}` | {total:,} | {missed:,} | {percent:5.1f}% |"
        )
    return lines


def _load_python_rows(
    report_path: pathlib.Path,
    repo_root: pathlib.Path,
) -> Tuple[List[Row], Dict[str, float]]:
    try:
        payload = json.loads(report_path.read_text(encoding="utf-8"))
    except FileNotFoundError as exc:
        raise SystemExit(f"Python coverage JSON not found: {report_path}") from exc

    repo_root = repo_root.resolve()
    rows: List[Row] = []

    for path_str, details in payload.get("files", {}).items():
        summary = (details.get("summary") or {})
        total = int(summary.get("num_statements", 0))
        missed = int(summary.get("missing_lines", 0))
        percent = float(summary.get("percent_covered", 0.0))

        if total == 0 and missed == 0:
            continue

        file_path = pathlib.Path(path_str)
        if not file_path.is_absolute():
            file_path = (repo_root / file_path).resolve()
        else:
            file_path = file_path.resolve()

        try:
            rel_path = file_path.relative_to(repo_root)
        except ValueError:
            continue

        rows.append((rel_path.as_posix(), total, missed, percent))

    rows.sort(key=lambda item: item[0])

    totals = payload.get("totals", {})
    return rows, {
        "total": float(totals.get("num_statements", 0)),
        "covered": float(totals.get("covered_lines", 0)),
        "missed": float(totals.get("missing_lines", 0)),
        "percent": float(totals.get("percent_covered", 0.0)),
    }


def _load_rust_rows(
    summary_path: pathlib.Path,
    repo_root: pathlib.Path,
) -> Tuple[List[Row], Dict[str, float]]:
    summary_result = load_rust_summary(summary_path, repo_root)
    if not isinstance(summary_result, tuple):
        summary_result = tuple(summary_result)
    if len(summary_result) < 2:
        raise SystemExit("Rust summary loader returned an unexpected payload")

    rows, totals = summary_result[:2]
    # Normalise totals dict to expected keys
    total = float(totals.get("count", 0))
    covered = float(totals.get("covered", 0))
    missed = float(totals.get("notcovered", total - covered))
    return rows, {
        "total": total,
        "covered": covered,
        "missed": missed,
        "percent": float(totals.get("percent", 0.0)),
    }


def _format_summary_block(
    heading: str,
    column_label: str,
    totals: Dict[str, float],
    rows: List[Row],
    max_rows: int,
) -> List[str]:
    display_rows, truncated = _select_rows(rows, max_rows)
    lines = [heading]
    total = int(totals.get("total", 0))
    covered = int(totals.get("covered", 0))
    missed = int(totals.get("missed", total - covered))
    percent = totals.get("percent", 0.0)
    lines.append(
        f"**{percent:0.1f}%** covered ({covered:,} / {total:,} | {missed:,} missed)"
    )
    lines.extend(
        _format_table(display_rows, ("File", column_label, "Miss", "Cover"))
    )
    if truncated:
        lines.append(
            f"_Showing top {max_rows} entries by missed lines (of {len(rows)} total)._"
        )
    return lines


def main(argv: Iterable[str] | None = None) -> int:
    args = parse_args(argv)
    repo_root = args.repo_root.resolve()

    rust_rows, rust_totals = _load_rust_rows(args.rust_summary, repo_root)
    python_rows, python_totals = _load_python_rows(args.python_json, repo_root)

    output_lines: List[str] = ["### Coverage Summary", ""]

    output_lines.extend(
        _format_summary_block(
            "**Rust (lines)**", "Lines", rust_totals, rust_rows, args.max_rows
        )
    )
    output_lines.extend(["", ""])
    output_lines.extend(
        _format_summary_block(
            "**Python (statements)**", "Stmts", python_totals, python_rows, args.max_rows
        )
    )
    output_lines.append("")
    output_lines.append(
        "_Generated automatically via `generate_coverage_comment.py`._"
    )

    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text("\n".join(output_lines) + "\n", encoding="utf-8")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
