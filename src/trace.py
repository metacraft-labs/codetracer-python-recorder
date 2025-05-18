import json
import os
import runpy
import sys
import types
from pathlib import Path
from typing import Any, Dict, List


class StdoutInterceptor:
    """Capture writes to stdout and emit trace events."""

    def __init__(self, tracer: "Tracer") -> None:
        self.tracer = tracer
        self.original = sys.stdout

    def write(self, text: str) -> int:
        self.original.write(text)
        if text and text != "\n":
            self.tracer.emit({"Event": {"kind": 0, "metadata": "", "content": text.rstrip("\n")}})
        return len(text)

    def flush(self) -> None:  # pragma: no cover - delegation
        self.original.flush()


class Tracer:
    """Runtime tracer producing CodeTracer traces."""

    def __init__(self, program: str) -> None:
        self.program_path = os.path.abspath(program)
        self.program_dir = str(Path(self.program_path).parent)
        self.events: List[Dict[str, Any]] = []
        self.paths: List[str] = []
        self.path_map: Dict[str, int] = {}
        self.var_map: Dict[str, int] = {}
        self.functions: Dict[types.CodeType, int] = {}
        self.types: Dict[str, int] = {}
        self.stdout = StdoutInterceptor(self)
        self._register_builtin_types()
        # top level
        self._register_path("")
        self._register_function("", 1, "<top-level>")
        self.emit({"Call": {"function_id": 0, "args": []}})

    # ------------------------------------------------------------------ utils
    def emit(self, event: Dict[str, Any]) -> None:
        self.events.append(event)

    # ------------------------------------------------------------------- types
    def _register_type(self, kind: int, name: str) -> int:
        if name not in self.types:
            type_id = len(self.types)
            self.types[name] = type_id
            self.emit({"Type": {"kind": kind, "lang_type": name, "specific_info": {"kind": "None"}}})
        return self.types[name]

    def _register_builtin_types(self) -> None:
        self._register_type(7, "Integer")
        self._register_type(9, "String")
        self._register_type(12, "Bool")
        self._register_type(9, "Symbol")
        self._register_type(24, "No type")
        self._register_type(8, "Float")
        self._register_type(27, "Tuple")
        self._register_type(16, "Bytes")
        self._register_type(6, "Complex")

    # -------------------------------------------------------------------- paths
    def _register_path(self, path: str) -> int:
        """Register a source file path and emit a ``Path`` event."""

        if path:
            abs_path = os.path.abspath(path)
            try:
                path = os.path.relpath(abs_path, self.program_dir)
            except ValueError:
                path = abs_path

        if path not in self.path_map:
            self.path_map[path] = len(self.paths)
            self.paths.append(path)
            self.emit({"Path": path})
        return self.path_map[path]

    # ---------------------------------------------------------------- functions
    def _register_function(self, path: str, line: int, name: str) -> int:
        key = (path, line, name)
        if key not in self.functions:
            func_id = len(self.functions)
            self.functions[key] = func_id
            self.emit({"Function": {"path_id": self._register_path(path), "line": line, "name": name}})
        return self.functions[key]

    def _register_functions_in_locals(self, frame: types.FrameType) -> None:
        for name, val in frame.f_locals.items():
            if isinstance(val, types.FunctionType):
                code = val.__code__
                if code.co_firstlineno == frame.f_lineno:
                    filename = os.path.abspath(code.co_filename)
                    self._register_function(filename, code.co_firstlineno, val.__name__)

    # -------------------------------------------------------------- variables
    def _var_id(self, name: str) -> int:
        if name not in self.var_map:
            self.var_map[name] = len(self.var_map)
            self.emit({"VariableName": name})
        return self.var_map[name]

    def _capture_locals(self, frame: types.FrameType) -> None:
        for name, val in frame.f_locals.items():
            if name.startswith("__"):
                continue
            if isinstance(val, types.FunctionType):
                continue
            vid = self._var_id(name)
            self.emit({"Value": {"variable_id": vid, "value": self._value(val)}})

    # ----------------------------------------------------------------- values
    def _ensure_type(self, kind: int, name: str) -> int:
        if name not in self.types:
            return self._register_type(kind, name)
        return self.types[name]

    def _value(self, val: Any) -> Dict[str, Any]:
        if isinstance(val, bool):
            return {"kind": "Bool", "type_id": self.types["Bool"], "b": val}
        if isinstance(val, int):
            return {"kind": "Int", "type_id": self.types["Integer"], "i": val}
        if isinstance(val, float):
            type_id = self._ensure_type(8, "Float")
            return {"kind": "Float", "type_id": type_id, "f": val}
        if isinstance(val, str):
            return {"kind": "String", "type_id": self.types["String"], "text": val}
        if isinstance(val, (bytes, bytearray)):
            type_id = self._ensure_type(16, "Bytes")
            return {"kind": "Raw", "type_id": type_id, "r": str(val)}
        if isinstance(val, list):
            type_id = self._ensure_type(0, "Array")
            return {
                "kind": "Sequence",
                "type_id": type_id,
                "elements": [self._value(v) for v in val],
                "is_slice": False,
            }
        if isinstance(val, tuple):
            type_id = self._ensure_type(27, "Tuple")
            return {
                "kind": "Tuple",
                "type_id": type_id,
                "elements": [self._value(v) for v in val],
            }
        if isinstance(val, complex):
            type_id = self._ensure_type(6, "Complex")
            return {
                "kind": "Struct",
                "type_id": type_id,
                "field_values": [self._value(val.real), self._value(val.imag)],
            }
        if val is None:
            return {"kind": "None", "type_id": self.types["No type"]}
        if hasattr(val, "__dict__"):
            type_id = self._ensure_type(6, val.__class__.__name__)
            fields = [
                self._value(v)
                for _, v in sorted(val.__dict__.items(), key=lambda kv: kv[0])
            ]
            return {
                "kind": "Struct",
                "type_id": type_id,
                "field_values": fields,
            }
        type_id = self._ensure_type(16, "Object")
        return {"kind": "Raw", "type_id": type_id, "r": str(val)}

    # --------------------------------------------------------------- callbacks
    def handle_line(self, frame: types.FrameType) -> None:
        path = os.path.abspath(frame.f_code.co_filename)
        path_id = self._register_path(path)
        self.emit({"Step": {"path_id": path_id, "line": frame.f_lineno}})
        self._register_functions_in_locals(frame)
        self._capture_locals(frame)

    def handle_call(self, frame: types.FrameType) -> None:
        code = frame.f_code
        filename = os.path.abspath(code.co_filename)
        func_id = self._register_function(filename, code.co_firstlineno, code.co_name)
        args: List[Dict[str, Any]] = []
        for name in code.co_varnames[: code.co_argcount]:
            if name in frame.f_locals:
                vid = self._var_id(name)
                args.append({"variable_id": vid, "value": self._value(frame.f_locals[name])})
        self.emit({"Call": {"function_id": func_id, "args": args}})

    def handle_return(self, frame: types.FrameType, arg: Any) -> None:
        path = os.path.abspath(frame.f_code.co_filename)
        path_id = self._register_path(path)
        self.emit({"Step": {"path_id": path_id, "line": frame.f_lineno}})
        self._capture_locals(frame)
        vid = self._var_id("<return_value>")
        value = self._value(arg)
        self.emit({"Value": {"variable_id": vid, "value": value}})
        self.emit({"Return": {"return_value": value}})


def trace_program(program: str) -> Tracer:
    tracer = Tracer(program)

    def global_trace(frame: types.FrameType, event: str, arg: Any):
        filename = os.path.abspath(frame.f_code.co_filename)
        if not filename.startswith(tracer.program_dir):
            return
        if event == "call":
            tracer.handle_call(frame)
            return local_trace

    def local_trace(frame: types.FrameType, event: str, arg: Any):
        filename = os.path.abspath(frame.f_code.co_filename)
        if not filename.startswith(tracer.program_dir):
            return
        if event == "line":
            tracer.handle_line(frame)
        elif event == "call":
            tracer.handle_call(frame)
            return local_trace
        elif event == "return":
            tracer.handle_return(frame, arg)
        return local_trace

    sys.settrace(global_trace)
    sys.stdout = tracer.stdout
    runpy.run_path(os.path.abspath(program), run_name="__main__")
    sys.settrace(None)
    sys.stdout = tracer.stdout.original
    return tracer


if __name__ == "__main__":
    if len(sys.argv) < 2:
        raise SystemExit("Usage: trace.py <program.py>")

    program_path = sys.argv[1]
    tracer = trace_program(program_path)

    meta = {"workdir": os.getcwd(), "program": sys.argv[0], "args": sys.argv[1:]}
    with open("trace_metadata.json", "w") as f:
        json.dump(meta, f, indent=2)
    with open("trace_paths.json", "w") as f:
        json.dump(tracer.paths, f, indent=2)
    with open("trace.json", "w") as f:
        json.dump(tracer.events, f, indent=2)
