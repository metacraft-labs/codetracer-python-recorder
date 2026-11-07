"""CLI entrypoint for the owned pure-Python reference recorder."""
import importlib.util
from pathlib import Path
import sys
from threading import RLock
from types import ModuleType
from typing import List, Optional

_REFERENCE_NAME = __package__ + "._owned_reference_trace"
_REFERENCE_LOCK = RLock()
_reference: Optional[ModuleType] = None


def _owned_reference() -> ModuleType:
    global _reference
    owned_path = Path(__file__).resolve().parent.parent / "trace.py"
    with _REFERENCE_LOCK:
        if not owned_path.is_file():
            raise ImportError(f"Owned recorder module is missing: {owned_path}")
        if _reference is not None:
            if Path(_reference.__file__).resolve() != owned_path:
                raise ImportError("Cached recorder module does not belong to this package")
            return _reference
        spec = importlib.util.spec_from_file_location(_REFERENCE_NAME, owned_path)
        if spec is None or spec.loader is None:
            raise ImportError(f"Cannot load owned recorder module: {owned_path}")
        module = importlib.util.module_from_spec(spec)
        previous = sys.modules.get(_REFERENCE_NAME)
        sys.modules[_REFERENCE_NAME] = module
        try:
            spec.loader.exec_module(module)
            if Path(module.__file__).resolve() != owned_path:
                raise ImportError("Loaded recorder module does not belong to this package")
            if not callable(getattr(module, "main", None)):
                raise ImportError("Owned recorder module has no callable main")
        except BaseException:
            if previous is None:
                sys.modules.pop(_REFERENCE_NAME, None)
            else:
                sys.modules[_REFERENCE_NAME] = previous
            raise
        _reference = module
        return module


def main(argv: Optional[List[str]] = None) -> None:
    return _owned_reference().main(sys.argv[1:] if argv is None else argv)
