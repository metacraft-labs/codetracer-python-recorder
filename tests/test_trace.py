import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
TRACE_SCRIPT = REPO_ROOT / "src" / "trace.py"
PROGRAMS_DIR = Path(__file__).parent / "programs"
FIXTURES_DIR = Path(__file__).parent / "fixtures"


class TraceTests(unittest.TestCase):
    def test_program_traces(self):
        for program in sorted(PROGRAMS_DIR.glob("*.py")):
            with self.subTest(program=program.name):
                with tempfile.TemporaryDirectory() as tmpdir:
                    subprocess.run(
                        [sys.executable, str(TRACE_SCRIPT), str(program)],
                        cwd=tmpdir,
                        check=True,
                    )
                    trace_file = Path(tmpdir) / "trace.json"
                    self.assertTrue(trace_file.exists())
                    with open(trace_file) as f:
                        trace_data = json.load(f)
                    trace_data.pop("workdir", None)
                    fixture_path = FIXTURES_DIR / f"{program.stem}.json"
                    self.assertTrue(
                        fixture_path.exists(),
                        msg=f"Missing fixture for {program.name}",
                    )
                    with open(fixture_path) as f:
                        expected_data = json.load(f)
                    expected_data.pop("workdir", None)
                    self.assertEqual(trace_data, expected_data)


if __name__ == "__main__":
    unittest.main()
