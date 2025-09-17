import json
import runpy
import tempfile
import unittest
from pathlib import Path
from typing import Dict, List, Tuple

import codetracer_python_recorder as codetracer


class LocalsSnapshotTests(unittest.TestCase):
    def _run_script(self, code: str) -> Tuple[List[str], Dict[str, List[Dict[str, int]]], Dict[int, str]]:
        with tempfile.TemporaryDirectory() as tmpdir:
            base = Path(tmpdir)
            script = base / "locals_demo.py"
            script.write_text(code)
            lines = script.read_text().splitlines()

            out_dir = base / "trace_out"
            out_dir.mkdir()

            session = codetracer.start(out_dir, format=codetracer.TRACE_JSON, start_on_enter=script)
            try:
                runpy.run_path(str(script), run_name="__main__")
            finally:
                codetracer.flush()
                codetracer.stop()

            events = json.loads((out_dir / "trace.json").read_text())
            paths = json.loads((out_dir / "trace_paths.json").read_text())

        varnames: Dict[int, str] = {}
        frames: Dict[str, List[Dict[str, int]]] = {}
        script_path_id = paths.index(str(script))
        current_line = None
        current_path = None

        for event in events:
            if "VariableName" in event:
                name = event["VariableName"]
                varnames[len(varnames)] = name
            elif "Step" in event:
                step = event["Step"]
                pid = int(step["path_id"])
                if pid == script_path_id:
                    current_path = pid
                    current_line = int(step["line"])
                else:
                    current_path = None
                    current_line = None
            elif "Value" in event and current_path == script_path_id and current_line is not None:
                payload = event["Value"]
                variable_id = int(payload["variable_id"])
                value = payload["value"]
                frames.setdefault(f"{current_line}", []).append({
                    "variable_id": variable_id,
                    "kind": value.get("kind"),
                    "int": value.get("i"),
                    "text": value.get("text"),
                })

        return lines, frames, varnames

    def _values_for(self, frames: Dict[str, List[Dict[str, int]]], varnames: Dict[int, str], line: str, name: str) -> List[Dict[str, int]]:
        vid = next(k for k, v in varnames.items() if v == name)
        return [entry for entry in frames.get(line, []) if entry["variable_id"] == vid]

    def test_function_locals_refresh_each_step(self) -> None:
        code = (
            "counter = 10\n"
            "\n"
            "def bump(n):\n"
            "    total = n\n"
            "    total += 1\n"
            "    shadow = total * 2\n"
            "    return total + shadow\n"
            "\n"
            "result = bump(counter)\n"
            "result += 5\n"
        )
        lines, frames, varnames = self._run_script(code)

        def find_line(text: str) -> int:
            for idx, line in enumerate(lines, start=1):
                if line.strip() == text:
                    return idx
            raise AssertionError(f"missing line: {text}")

        total_update_line = str(find_line("total += 1"))
        shadow_eval_line = str(find_line("shadow = total * 2"))
        return_line = str(find_line("return total + shadow"))
        module_update = str(find_line("result += 5"))

        total_initial_values = self._values_for(frames, varnames, total_update_line, "total")
        total_updated_values = self._values_for(frames, varnames, shadow_eval_line, "total")
        shadow_values = self._values_for(frames, varnames, return_line, "shadow")

        self.assertTrue(total_initial_values, "expected locals snapshot before total += 1 executes")
        self.assertTrue(total_updated_values, "expected locals snapshot after total += 1 executes")
        self.assertTrue(shadow_values, "expected locals snapshot reflecting shadow")

        self.assertEqual(total_initial_values[-1]["kind"], "Int")
        self.assertEqual(total_initial_values[-1]["int"], 10)
        self.assertEqual(total_updated_values[-1]["int"], 11)
        self.assertEqual(shadow_values[-1]["int"], 22)

        result_values = self._values_for(frames, varnames, module_update, "result")
        self.assertTrue(result_values, "module locals should include result binding before update")
        self.assertEqual(result_values[-1]["int"], 33)

        builtins_seen = any(name == "__builtins__" for name in varnames.values())
        self.assertFalse(builtins_seen, "__builtins__ should be filtered from locals snapshots")

    def test_generator_locals_record_yields_and_loop_state(self) -> None:
        code = (
            "def produce():\n"
            "    acc = 0\n"
            "    for item in range(3):\n"
            "        acc += item\n"
            "        note = f'{item}:{acc}'\n"
            "        yield (item, acc, note)\n"
            "    return acc\n"
            "\n"
            "g = produce()\n"
            "first = next(g)\n"
            "second = next(g)\n"
        )
        lines, frames, varnames = self._run_script(code)

        def find_line(text: str) -> int:
            for idx, line in enumerate(lines, start=1):
                if line.strip() == text:
                    return idx
            raise AssertionError(f"missing line: {text}")

        item_update_line = str(find_line("acc += item"))
        yield_line = str(find_line("yield (item, acc, note)"))

        item_values = self._values_for(frames, varnames, item_update_line, "item")
        note_values = self._values_for(frames, varnames, yield_line, "note")

        self.assertGreaterEqual(len(item_values), 2, "expected multiple loop iterations to surface")
        self.assertGreaterEqual(len(note_values), 2, "expected multiple yield snapshots")

        seen_items = {entry["int"] for entry in item_values if entry.get("kind") == "Int"}
        seen_notes = {entry["text"] for entry in note_values if entry.get("kind") == "String"}

        self.assertIn(0, seen_items)
        self.assertIn(1, seen_items)
        self.assertIn("0:0", seen_notes)
        self.assertIn("1:1", seen_notes)


if __name__ == "__main__":
    unittest.main()
