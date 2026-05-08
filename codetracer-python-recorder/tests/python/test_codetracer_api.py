import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

import codetracer_python_recorder as codetracer


class TracingApiTests(unittest.TestCase):
    def setUp(self) -> None:  # ensure clean state before each test
        codetracer.stop()

    def test_start_stop_and_status(self) -> None:
        with tempfile.TemporaryDirectory() as tmpdir:
            trace_dir = Path(tmpdir)
            session = codetracer.start(trace_dir)
            self.assertTrue(codetracer.is_tracing())
            self.assertIsInstance(session, codetracer.TraceSession)
            self.assertEqual(session.path, trace_dir)
            self.assertEqual(session.format, codetracer.DEFAULT_FORMAT)
            codetracer.flush()  # should not raise
            session.flush()  # same
            session.stop()
            self.assertFalse(codetracer.is_tracing())

    def test_context_manager(self) -> None:
        with tempfile.TemporaryDirectory() as tmpdir:
            trace_dir = Path(tmpdir)
            with codetracer.trace(trace_dir) as session:
                self.assertTrue(codetracer.is_tracing())
                self.assertIsInstance(session, codetracer.TraceSession)
            self.assertFalse(codetracer.is_tracing())

    def test_start_emits_trace_files(self) -> None:
        with tempfile.TemporaryDirectory() as tmpdir:
            trace_dir = Path(tmpdir)
            codetracer.start(trace_dir)
            # Execute a small workload to ensure callbacks fire.
            def _workload() -> int:
                return sum(range(5))

            self.assertEqual(_workload(), 10)
            codetracer.stop()

            # Per Recorder-CLI-Conventions.md §4 the recorder is CTFS-only.
            # The Nim writer emits a single multi-stream ``<program>.ct``
            # container; the JSON sidecar files (``trace_metadata.json``,
            # ``trace_paths.json``) only exist for the legacy JSON format
            # path (kept around for API-level event-stream tests).
            ct_files = list(trace_dir.glob("*.ct"))
            self.assertTrue(
                ct_files,
                f"expected at least one CTFS .ct file in {trace_dir}; "
                f"contents: {list(trace_dir.iterdir())}",
            )

    def test_environment_auto_start(self) -> None:
        script = "import codetracer_python_recorder as codetracer, sys; sys.stdout.write(str(codetracer.is_tracing()))"
        with tempfile.TemporaryDirectory() as tmpdir:
            env = os.environ.copy()
            env["CODETRACER_TRACE"] = str(Path(tmpdir))
            out = subprocess.check_output([sys.executable, "-c", script], env=env)
            self.assertEqual(out.decode(), "True")

    def test_start_rejects_unsupported_format(self) -> None:
        with tempfile.TemporaryDirectory() as tmpdir:
            with self.assertRaises(ValueError):
                codetracer.start(Path(tmpdir), format="yaml")
        self.assertFalse(codetracer.is_tracing())

    def test_start_rejects_file_path(self) -> None:
        with tempfile.TemporaryDirectory() as tmpdir:
            file_path = Path(tmpdir) / "trace.bin"
            file_path.write_text("placeholder")
            with self.assertRaises(ValueError):
                codetracer.start(file_path)
        self.assertFalse(codetracer.is_tracing())


if __name__ == "__main__":
    unittest.main()
