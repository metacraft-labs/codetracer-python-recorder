from __future__ import annotations

import importlib
import json
import math
import os
import sys
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Callable, Iterable, Sequence
import textwrap

import pytest

from codetracer_python_recorder import trace

CALLS_PER_BATCH = 1_000
LOCALS_PER_CALL = 50
FUNCTIONS_PER_MODULE = 10
SERVICES_MODULES = 6
WORKER_MODULES = 3
EXTERNAL_MODULES = 1
UNIQUE_CODE_OBJECTS = (
    SERVICES_MODULES + WORKER_MODULES + EXTERNAL_MODULES
) * FUNCTIONS_PER_MODULE

MAX_RUNTIME_RATIO = {
    "glob": 60.0,
    "regex": 30.0,
}

_SKIP_REASON = (
    "trace filter perf smoke disabled; set CODETRACER_TRACE_FILTER_PERF=1 to enable"
)
pytestmark = pytest.mark.skipif(
    os.environ.get("CODETRACER_TRACE_FILTER_PERF") != "1", reason=_SKIP_REASON
)


@dataclass(frozen=True)
class ModuleSpec:
    relative_path: str
    module_name: str
    func_prefix: str
    functions: int


@dataclass(frozen=True)
class PerfScenario:
    label: str
    filter_path: Path


@dataclass(frozen=True)
class PerfResult:
    label: str
    duration_seconds: float
    filter_names: list[str]
    scopes_skipped: int
    value_redactions: dict[str, int]

    def to_dict(self) -> dict[str, object]:
        payload: dict[str, object] = {
            "label": self.label,
            "duration_seconds": self.duration_seconds,
            "filter_names": list(self.filter_names),
            "scopes_skipped": self.scopes_skipped,
            "value_redactions": dict(self.value_redactions),
        }
        return payload


@dataclass
class PerfDataset:
    functions: list[Callable[[int], int]]
    event_indices: list[int]
    imported_modules: set[str]
    imported_packages: set[str]


class PerfWorkspace:
    def __init__(self, root: Path) -> None:
        self.root = root
        self.project_root = root / "project"
        self.project_root.mkdir(parents=True, exist_ok=True)
        self.filters_dir = self.project_root / ".codetracer"
        self.filters_dir.mkdir(parents=True, exist_ok=True)

        self._filters = FilterFiles.create(self.filters_dir)
        self.dataset = self._build_dataset()
        self.scenarios = self._build_scenarios()

    def cleanup(self) -> None:
        for name in sorted(
            self.dataset.imported_modules | self.dataset.imported_packages,
            key=len,
            reverse=True,
        ):
            sys.modules.pop(name, None)

    def _build_scenarios(self) -> list[PerfScenario]:
        return [
            PerfScenario("baseline", self._filters.baseline),
            PerfScenario("glob", self._filters.glob),
            PerfScenario("regex", self._filters.regex),
        ]

    def _build_dataset(self) -> PerfDataset:
        local_names = build_local_names()
        specs = build_module_specs()
        functions: list[Callable[[int], int]] = []
        for spec in specs:
            relative = Path(spec.relative_path)
            self._ensure_package_inits(relative)
            module_path = self.project_root / relative
            module_path.parent.mkdir(parents=True, exist_ok=True)
            module_path.write_text(
                module_source(spec.func_prefix, spec.functions, local_names),
                encoding="utf-8",
            )

        sys.path.insert(0, str(self.project_root))
        imported_modules: set[str] = set()
        imported_packages: set[str] = set()
        try:
            for spec in specs:
                module = importlib.import_module(spec.module_name)
                imported_modules.update(_module_lineage(spec.module_name))
                for idx in range(spec.functions):
                    func_name = f"{spec.func_prefix}_{idx}"
                    func = getattr(module, func_name)
                    functions.append(func)
        finally:
            sys.path.pop(0)

        if len(functions) != UNIQUE_CODE_OBJECTS:
            raise AssertionError(
                f"expected {UNIQUE_CODE_OBJECTS} code objects, found {len(functions)}"
            )

        # Collect package lineage for cleanup (parents only; module entries already captured).
        for name in imported_modules:
            parts = name.split(".")
            imported_packages.update(".".join(parts[:idx]) for idx in range(1, len(parts)))

        event_indices = [i % len(functions) for i in range(CALLS_PER_BATCH)]
        return PerfDataset(
            functions=functions,
            event_indices=event_indices,
            imported_modules=imported_modules,
            imported_packages=imported_packages,
        )

    def _ensure_package_inits(self, relative_path: Path) -> None:
        current = self.project_root
        parts = relative_path.parts[:-1]
        for part in parts:
            current = current / part
            current.mkdir(parents=True, exist_ok=True)
            init_file = current / "__init__.py"
            if not init_file.exists():
                init_file.write_text("", encoding="utf-8")


@dataclass
class FilterFiles:
    baseline: Path
    glob: Path
    regex: Path

    @classmethod
    def create(cls, root: Path) -> FilterFiles:
        baseline = root / "bench-baseline.toml"
        glob = root / "bench-glob.toml"
        regex = root / "bench-regex.toml"

        baseline.write_text(baseline_config(), encoding="utf-8")
        glob.write_text(glob_config(), encoding="utf-8")
        regex.write_text(regex_config(), encoding="utf-8")

        return cls(baseline=baseline, glob=glob, regex=regex)


def test_trace_filter_perf_smoke(tmp_path: Path) -> None:
    workspace = PerfWorkspace(tmp_path)
    results: list[PerfResult] = []
    try:
        for scenario in workspace.scenarios:
            results.append(run_scenario(workspace, scenario))

        baseline = _result_by_label(results, "baseline")
        glob = _result_by_label(results, "glob")
        regex = _result_by_label(results, "regex")

        assert baseline.duration_seconds > 0
        assert glob.duration_seconds > 0
        assert regex.duration_seconds > 0

        assert baseline.filter_names == ["bench-baseline"]
        assert "bench-glob" in glob.filter_names
        assert "bench-regex" in regex.filter_names

        assert glob.scopes_skipped > 0

        assert baseline.value_redactions.get("local", 0) == 0
        assert glob.value_redactions.get("local", 0) > 0
        assert regex.value_redactions.get("local", 0) > 0

        baseline_time = baseline.duration_seconds
        assert baseline_time > 0 and math.isfinite(baseline_time)

        for label, limit in MAX_RUNTIME_RATIO.items():
            candidate = _result_by_label(results, label)
            ceiling = baseline_time * limit + 0.5
            assert candidate.duration_seconds <= ceiling, (
                f"{label} scenario exceeded runtime ceiling "
                f"{candidate.duration_seconds:.4f}s > {ceiling:.4f}s "
                f"(baseline {baseline_time:.4f}s, limit {limit}x)"
            )
    finally:
        workspace.cleanup()
        _maybe_write_results(results)


def run_scenario(workspace: PerfWorkspace, scenario: PerfScenario) -> PerfResult:
    dataset = workspace.dataset
    trace_dir = workspace.root / f"trace-{scenario.label}"
    with trace(
        trace_dir,
        format="json",
        trace_filter=str(scenario.filter_path),
    ):
        prewarm_dataset(dataset)
        start = time.perf_counter()
        run_workload(dataset)
        duration = time.perf_counter() - start

    metadata = _load_metadata(trace_dir)
    filter_meta = metadata.get("trace_filter", {}) if metadata else {}
    filters = filter_meta.get("filters") or []
    filter_names = [
        entry.get("name")  # type: ignore[union-attr]
        for entry in filters
        if isinstance(entry, dict) and entry.get("name")
    ]
    stats = filter_meta.get("stats") or {}
    scopes_skipped = int(stats.get("scopes_skipped") or 0)
    value_redactions_obj = stats.get("value_redactions") or {}
    value_redactions = {
        key: int(value)
        for key, value in value_redactions_obj.items()
        if isinstance(key, str)
    }

    return PerfResult(
        label=scenario.label,
        duration_seconds=duration,
        filter_names=filter_names,
        scopes_skipped=scopes_skipped,
        value_redactions=value_redactions,
    )


def prewarm_dataset(dataset: PerfDataset) -> None:
    for func in dataset.functions:
        func(0)


def run_workload(dataset: PerfDataset) -> None:
    functions = dataset.functions
    for index in dataset.event_indices:
        functions[index](index)


def _load_metadata(trace_dir: Path) -> dict[str, object]:
    metadata_path = trace_dir / "trace_metadata.json"
    if not metadata_path.exists():
        raise AssertionError(f"trace metadata not generated for {trace_dir}")
    return json.loads(metadata_path.read_text(encoding="utf-8"))


def _result_by_label(results: Sequence[PerfResult], label: str) -> PerfResult:
    for entry in results:
        if entry.label == label:
            return entry
    raise AssertionError(f"missing result for {label!r}")


def _maybe_write_results(results: Sequence[PerfResult]) -> None:
    destination = os.environ.get("CODETRACER_TRACE_FILTER_PERF_OUTPUT")
    if not destination or not results:
        return

    output_path = Path(destination)
    output_path.parent.mkdir(parents=True, exist_ok=True)

    baseline = next((r for r in results if r.label == "baseline"), None)
    baseline_time = baseline.duration_seconds if baseline else None

    payload = {
        "calls_per_batch": CALLS_PER_BATCH,
        "locals_per_call": LOCALS_PER_CALL,
        "results": [
            {
                **result.to_dict(),
                "relative_to_baseline": (
                    result.duration_seconds / baseline_time
                    if baseline_time and baseline_time > 0
                    else None
                ),
            }
            for result in results
        ],
    }
    output_path.write_text(json.dumps(payload, indent=2), encoding="utf-8")


def build_module_specs() -> list[ModuleSpec]:
    specs: list[ModuleSpec] = []
    for idx in range(SERVICES_MODULES):
        specs.append(
            ModuleSpec(
                relative_path=f"bench_pkg/services/api/module_{idx}.py",
                module_name=f"bench_pkg.services.api.module_{idx}",
                func_prefix=f"api_handler_{idx}",
                functions=FUNCTIONS_PER_MODULE,
            )
        )
    for idx in range(WORKER_MODULES):
        specs.append(
            ModuleSpec(
                relative_path=f"bench_pkg/jobs/worker/module_{idx}.py",
                module_name=f"bench_pkg.jobs.worker.module_{idx}",
                func_prefix=f"worker_task_{idx}",
                functions=FUNCTIONS_PER_MODULE,
            )
        )
    for idx in range(EXTERNAL_MODULES):
        specs.append(
            ModuleSpec(
                relative_path=f"bench_pkg/external/integration_{idx}.py",
                module_name=f"bench_pkg.external.integration_{idx}",
                func_prefix=f"integration_op_{idx}",
                functions=FUNCTIONS_PER_MODULE,
            )
        )
    return specs


def module_source(func_prefix: str, function_count: int, local_names: Sequence[str]) -> str:
    lines: list[str] = []
    for index in range(function_count):
        func_name = f"{func_prefix}_{index}"
        lines.append(f"def {func_name}(value):")
        for offset, name in enumerate(local_names):
            lines.append(f"    {name} = value + {offset}")
        lines.append("    return value")
        lines.append("")
    return "\n".join(lines)


def build_local_names() -> list[str]:
    names: list[str] = []
    for idx in range(15):
        names.append(f"public_field_{idx}")
    for idx in range(15):
        names.append(f"secret_field_{idx}")
    for idx in range(10):
        names.append(f"token_{idx}")
    names.extend(
        [
            "password_hash",
            "api_key",
            "credit_card",
            "session_id",
            "metric_latency",
            "metric_throughput",
            "metric_error_rate",
            "masked_value",
            "debug_flag",
            "trace_id",
        ]
    )
    if len(names) != LOCALS_PER_CALL:
        raise AssertionError(
            f"expected {LOCALS_PER_CALL} local names, found {len(names)}"
        )
    return names


def baseline_config() -> str:
    return textwrap.dedent(
        """
        [meta]
        name = "bench-baseline"
        version = 1
        description = "Tracing baseline without additional filter overhead."

        [scope]
        default_exec = "trace"
        default_value_action = "allow"
        """
    ).strip()


def glob_config() -> str:
    return textwrap.dedent(
        """
        [meta]
        name = "bench-glob"
        version = 1
        description = "Glob-heavy rule set for microbenchmark coverage."

        [scope]
        default_exec = "trace"
        default_value_action = "allow"

        [[scope.rules]]
        selector = "pkg:bench_pkg.services.api.*"
        value_default = "deny"
        reason = "Redact service locals except approved public fields"
        [[scope.rules.value_patterns]]
        selector = "local:glob:public_*"
        action = "allow"
        [[scope.rules.value_patterns]]
        selector = "local:glob:metric_*"
        action = "allow"
        [[scope.rules.value_patterns]]
        selector = "local:glob:secret_*"
        action = "deny"
        [[scope.rules.value_patterns]]
        selector = "local:glob:token_*"
        action = "deny"
        [[scope.rules.value_patterns]]
        selector = "local:glob:masked_*"
        action = "allow"
        [[scope.rules.value_patterns]]
        selector = "local:glob:password_*"
        action = "deny"

        [[scope.rules]]
        selector = "file:glob:bench_pkg/jobs/worker/module_*.py"
        exec = "skip"
        reason = "Disable redundant worker instrumentation"

        [[scope.rules]]
        selector = "pkg:bench_pkg.external.integration_*"
        value_default = "deny"
        [[scope.rules.value_patterns]]
        selector = "local:glob:metric_*"
        action = "allow"
        [[scope.rules.value_patterns]]
        selector = "local:glob:public_*"
        action = "allow"
        """
    ).strip()


def regex_config() -> str:
    return textwrap.dedent(
        """
        [meta]
        name = "bench-regex"
        version = 1
        description = "Regex-heavy rule set for microbenchmark coverage."

        [scope]
        default_exec = "trace"
        default_value_action = "allow"

        [[scope.rules]]
        selector = 'pkg:regex:^bench_pkg\\.services\\.api\\.module_\\d+$'
        value_default = "deny"
        reason = "Regex match on service modules"
        [[scope.rules.value_patterns]]
        selector = 'local:regex:^(public|metric)_\\w+$'
        action = "allow"
        [[scope.rules.value_patterns]]
        selector = 'local:regex:^(secret|token)_\\w+$'
        action = "deny"
        [[scope.rules.value_patterns]]
        selector = 'local:regex:^(password|api|credit|session)_.*$'
        action = "deny"

        [[scope.rules]]
        selector = 'file:regex:^bench_pkg/jobs/worker/module_\\d+\\.py$'
        exec = "skip"
        reason = "Regex skip for worker modules"

        [[scope.rules]]
        selector = 'obj:regex:^bench_pkg\\.external\\.integration_\\d+\\.integration_op_\\d+$'
        value_default = "deny"
        [[scope.rules.value_patterns]]
        selector = 'local:regex:^masked_.*$'
        action = "allow"
        [[scope.rules.value_patterns]]
        selector = 'local:regex:^metric_.*$'
        action = "allow"
        """
    ).strip()


def _module_lineage(name: str) -> Iterable[str]:
    parts = name.split(".")
    return (".".join(parts[:idx]) for idx in range(1, len(parts) + 1))
