"""Integration tests for runtime IO capture."""

from __future__ import annotations

import base64
import json
import os
import subprocess
import sys
import textwrap
from collections import defaultdict
from pathlib import Path
from typing import DefaultDict, Iterable, Tuple


def _load_io_events(trace_file: Path) -> Iterable[Tuple[int, dict[str, object], bytes]]:
    events = json.loads(trace_file.read_text())
    for entry in events:
        payload = entry.get("Event")
        if not payload:
            continue
        metadata = json.loads(payload["metadata"])
        chunk = base64.b64decode(payload["content"])
        yield payload["kind"], metadata, chunk


def test_io_capture_records_all_streams(tmp_path: Path) -> None:
    script = textwrap.dedent(
        """
        import sys
        import codetracer_python_recorder as codetracer

        sys.stdout.write("hello stdout\\n")
        sys.stdout.flush()
        sys.stderr.write("warning\\n")
        sys.stderr.flush()
        data = sys.stdin.readline()
        sys.stdout.write(f"input={data}")
        sys.stdout.flush()
        codetracer.stop()
        """
    )

    env = os.environ.copy()
    env["CODETRACER_TRACE"] = str(tmp_path)
    env["CODETRACER_FORMAT"] = "json"

    completed = subprocess.run(
        [sys.executable, "-c", script],
        input="feed\n",
        text=True,
        capture_output=True,
        env=env,
        check=True,
    )

    assert completed.stdout == "hello stdout\ninput=feed\n"
    assert completed.stderr == "warning\n"

    trace_file = tmp_path / "trace.json"
    assert trace_file.exists(), "expected trace artefact"

    buffers: DefaultDict[str, bytearray] = defaultdict(bytearray)

    for kind, metadata, chunk in _load_io_events(trace_file):
        stream = metadata["stream"]
        buffers[stream].extend(chunk)

        assert metadata["byte_len"] == len(chunk)
        assert metadata["timestamp_ns"] >= 0
        assert isinstance(metadata["thread_id"], str)
        snapshot = metadata.get("snapshot")
        if snapshot is not None:
            assert {"path_id", "line", "frame_id"}.issubset(snapshot)

        if stream == "stdout":
            assert kind == 0  # EventLogKind::Write
        elif stream == "stderr":
            assert kind == 2  # EventLogKind::WriteOther
        elif stream == "stdin":
            assert kind == 3  # EventLogKind::Read

    assert buffers["stdout"].decode() == "hello stdout\ninput=feed\n"
    assert buffers["stderr"].decode() == "warning\n"
    assert buffers["stdin"].decode() == "feed\n"
