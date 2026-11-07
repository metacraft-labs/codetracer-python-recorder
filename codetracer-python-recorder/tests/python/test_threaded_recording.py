"""Recording a program while two of its threads run traced code.

Every ``sys.monitoring`` callback takes the recorder's process-wide tracer lock
and, while holding it, may run Python code: capturing a value whose type the
encoder does not know calls its ``__str__``.  Python code can give up the GIL
(a blocking call, or simply the interpreter's switch interval), and another
thread then runs until its own next monitoring event.  That thread must wait
for the tracer lock WITHOUT keeping the GIL, or the first thread can never take
the GIL back to finish its callback and release the lock: the whole process
hangs.

The program below makes that interleaving certain rather than lucky: the main
thread's local ``s`` has a ``__str__`` that sleeps (the sleep releases the GIL),
and a worker thread is executing traced lines the whole time.  It runs in a
subprocess under a timeout so a hang fails the test instead of the test run.

No mocks: the real recorder records a real script, and the container is decoded
by ``ct-print``, the canonical CTFS reader.
"""
from __future__ import annotations

import os
import subprocess
import sys
from pathlib import Path

from .support.ctfs import find_ct_file, parse_ctfs_trace

REPO_ROOT = Path(__file__).resolve().parents[2]

PROGRAM = """\
import threading
import time

stop = False


class SlowStr:
    def __str__(self):
        time.sleep(0.3)
        return "slow"


def worker(ready):
    n = 0
    ready.set()
    while not stop:
        n += 1
    return n


def main():
    global stop
    ready = threading.Event()
    t = threading.Thread(target=worker, args=(ready,))
    t.start()
    ready.wait()
    s = SlowStr()
    y = 2
    stop = True
    t.join()
    print("done", y, type(s).__name__)


main()
"""

# Unrecorded, the program finishes in well under a second.  Recorded, the busy
# worker makes it slower, but nowhere near this; a hang is the only way to hit it.
TIMEOUT_SECONDS = 60


def test_callback_that_releases_the_gil_does_not_hang_another_traced_thread(
    tmp_path: Path,
) -> None:
    script = tmp_path / "gil_release.py"
    script.write_text(PROGRAM, encoding="utf-8")
    out_dir = tmp_path / "trace"

    env = os.environ.copy()
    pythonpath = env.get("PYTHONPATH", "")
    env["PYTHONPATH"] = (
        str(REPO_ROOT) if not pythonpath else os.pathsep.join([str(REPO_ROOT), pythonpath])
    )
    env.pop("CODETRACER_PYTHON_RECORDER_OUT_DIR", None)
    env.pop("CODETRACER_PYTHON_RECORDER_DISABLED", None)

    try:
        proc = subprocess.run(
            [
                sys.executable,
                "-m",
                "codetracer_python_recorder",
                "--out-dir",
                str(out_dir),
                str(script),
            ],
            cwd=tmp_path,
            env=env,
            capture_output=True,
            text=True,
            timeout=TIMEOUT_SECONDS,
        )
    except subprocess.TimeoutExpired as exc:
        raise AssertionError(
            f"the recorded program hung for {TIMEOUT_SECONDS}s: a thread waiting for "
            "the tracer lock kept the GIL that the lock's holder needs to finish"
        ) from exc

    assert proc.returncode == 0, (proc.stdout, proc.stderr)
    assert "done 2 SlowStr" in proc.stdout, (proc.stdout, proc.stderr)

    parsed = parse_ctfs_trace(find_ct_file(out_dir))
    called = {parsed.functions[fid]["name"] for fid in parsed.calls}
    # Both threads were recorded, and the value whose capture released the GIL
    # did not cost the main thread its remaining steps.  (``__str__`` itself
    # runs inside the recorder's callback, where no events are delivered.)
    assert {"main", "worker"} <= called, sorted(called)
    script_path_ids = {
        index for index, path in enumerate(parsed.paths) if Path(path).name == script.name
    }
    recorded_lines = {line for path_id, line in parsed.steps if path_id in script_path_ids}
    assert {28, 29, 30, 31} <= recorded_lines, sorted(recorded_lines)
