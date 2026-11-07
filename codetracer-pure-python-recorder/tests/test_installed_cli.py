"""Real installed-wheel CLI boundaries, with no mocks.

The fixture deliberately builds the owning setuptools wheel and installs that
exact artifact into a genuine isolated environment using the declared SDK's
native UV frontend. This covers physical module ownership and console scripts
that editable/source imports cannot prove. All temporary production/mutation
artifacts are private to this fixture and finally restored; no original tests
or expected recorder events are filtered or replaced.
"""
import hashlib
import json
import os
from pathlib import Path
import subprocess
import shutil
import tempfile
import tomllib
import unittest

PACKAGE_ROOT = Path(__file__).resolve().parents[1]
REPO_ROOT = PACKAGE_ROOT.parent


class InstalledCliTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.sdk = Path(os.environ["RECORDER_TEST_BUILD_SDK"]).resolve()
        cls.sdk.relative_to("/nix/store")
        cls.identity = json.loads((cls.sdk / "codetracer-python-sdk-identity.json").read_text())
        sources = ["tools/python-sdk/default.nix", "flake.lock", "nix/python.nix", ".python-version"]
        encoded = json.dumps([(REPO_ROOT / p).read_bytes().decode("utf-8") for p in sources],
                             ensure_ascii=False, separators=(",", ":")).encode("utf-8")
        if cls.identity["sourceId"] != hashlib.sha256(encoded).hexdigest():
            raise AssertionError("SDK source identity does not match the owning source bytes")
        for key in ["underlyingPython", "underlyingUv", "underlyingMaturin"]:
            Path(cls.identity[key]).resolve().relative_to("/nix/store")
        sdk_python = cls.sdk / "bin/python"
        version = subprocess.check_output([str(sdk_python), "--version"], text=True).strip()
        if version != "Python " + cls.identity["pythonVersion"] or not version.startswith("Python 3.12."):
            raise AssertionError(version)
        for name, field in [("uv", "uvVersion"), ("maturin", "maturinVersion")]:
            version = subprocess.check_output([str(cls.sdk / "bin" / name), "--version"], text=True).strip()
            if version.split()[1] != cls.identity[field]:
                raise AssertionError((name, version, cls.identity[field]))
        scratch = REPO_ROOT / ".repro/build/python-installed-wheel-tests"
        scratch.mkdir(parents=True, exist_ok=True)
        cls.temporary = tempfile.TemporaryDirectory(dir=scratch)
        cls.addClassCleanup(cls.temporary.cleanup)
        cls.root = Path(cls.temporary.name)
        cls.environment = dict(os.environ, PYTHONDONTWRITEBYTECODE="1")
        cls.uv = str(cls.sdk / "bin/uv")
        # Stage every tracked owning package file byte-for-byte. Source-owned
        # build/egg-info artifacts from setuptools are then confined to TempDir.
        cls.staged_package = cls.root / PACKAGE_ROOT.name
        tracked = subprocess.check_output(["git", "-C", str(REPO_ROOT), "ls-files", "-z", "--", PACKAGE_ROOT.name]).decode().split("\0")
        manifest = {}
        for relative in [p for p in tracked if p]:
            source = REPO_ROOT / relative
            destination = cls.staged_package / source.relative_to(PACKAGE_ROOT)
            destination.parent.mkdir(parents=True, exist_ok=True)
            if source.is_symlink():
                # Reject external/absolute source escape rather than borrowing it.
                target = os.readlink(source)
                if Path(target).is_absolute():
                    raise AssertionError((relative, "absolute symlink"))
                source.resolve().relative_to(PACKAGE_ROOT.resolve())
                destination.symlink_to(target)
                manifest[relative] = {"symlink": os.readlink(source)}
            else:
                shutil.copy2(source, destination)
                before = hashlib.sha256(source.read_bytes()).hexdigest()
                if hashlib.sha256(destination.read_bytes()).hexdigest() != before:
                    raise AssertionError(relative)
                manifest[relative] = {"sha256": before}
        if not manifest:
            raise AssertionError("No tracked owning package inputs were staged")
        (cls.root / "source-manifest.json").write_text(json.dumps(manifest, indent=2))
        runtime_code = "import importlib.metadata,json,setuptools,wheel;print(json.dumps({'setuptoolsActual':importlib.metadata.version('setuptools'),'wheelActual':importlib.metadata.version('wheel'),'setuptoolsModule':setuptools.__file__,'wheelModule':wheel.__file__}))"
        actual = json.loads(cls.run_required([str(sdk_python), "-c", runtime_code]).stdout)
        underlying = json.loads(cls.run_required([cls.identity["underlyingPython"], "-c", runtime_code]).stdout)
        if actual != underlying:
            raise AssertionError("Wrapper does not preserve the declared backend interpreter closure")
        for field in ["setuptoolsModule", "wheelModule"]:
            Path(actual[field]).resolve().relative_to("/nix/store")
        (cls.root / "sdk-runtime.json").write_text(json.dumps({"declared": cls.identity, "actual": actual}, indent=2))
        cls.run_required([cls.uv, "build", str(cls.staged_package), "--wheel", "--out-dir", str(cls.root / "wheels"),
                          "--python", str(sdk_python), "--no-build-isolation", "--no-managed-python", "--offline"])
        wheels = list((cls.root / "wheels").glob("*.whl"))
        if len(wheels) != 1:
            raise AssertionError(wheels)
        version = tomllib.loads((PACKAGE_ROOT / "pyproject.toml").read_text())["project"]["version"]
        if wheels[0].name != f"codetracer_pure_python_recorder-{version}-py3-none-any.whl":
            raise AssertionError(wheels[0].name)
        cls.run_required([cls.uv, "venv", str(cls.root / "venv"), "--python", str(sdk_python),
                          "--no-managed-python", "--offline"])
        cls.python = cls.root / "venv/bin/python"
        cls.run_required([cls.uv, "pip", "install", "--python", str(cls.python), "--no-deps", "--no-build",
                          "--offline", str(wheels[0])])
        cls.program = cls.root / "only-program/program.py"
        cls.program.parent.mkdir()
        cls.program.write_text("value = 21\nprint(value * 2)\n")
        cls.reference = PACKAGE_ROOT / "src/trace.py"
        cls.owned_trace = Path(cls.run_required([str(cls.python), "-c",
            "from codetracer_pure_python_recorder import cli;from pathlib import Path;print(Path(cli.__file__).resolve().parent.parent/'trace.py')"]).stdout.strip())
        cls.owned_cli = Path(cls.run_required([str(cls.python), "-c",
            "from codetracer_pure_python_recorder import cli;print(cli.__file__)"]).stdout.strip())

    @classmethod
    def run_required(cls, args, cwd=None):
        result = subprocess.run(args, cwd=cwd, env=getattr(cls, "environment", os.environ),
                                text=True, capture_output=True)
        if result.returncode:
            raise AssertionError((args, result.returncode, result.stdout, result.stderr))
        return result

    def record(self, command, label, expected_program, args=None):
        cwd = self.root / label
        cwd.mkdir()
        args = [str(self.program)] if args is None else args
        result = self.run_required(command + args, cwd=cwd)
        self.assertEqual(result.stdout, "42\n")
        self.assertEqual(result.stderr, "")
        self.assertEqual(json.loads((cwd / "trace_metadata.json").read_text()),
                         {"workdir": str(cwd), "program": expected_program, "args": args})
        return tuple(json.loads((cwd / name).read_text()) for name in ["trace.json", "trace_paths.json"])

    def test_both_installed_scripts_preserve_complete_reference(self):
        expected = self.record([str(self.python), str(self.reference)], "direct", str(self.reference))
        for name in ["codetracer-record", "codetracer-record-pure"]:
            script = str(self.python.parent / name)
            self.assertEqual(self.record([script], name, script), expected)

    def test_cold_concurrent_private_import_and_cached_reuse(self):
        code = """import concurrent.futures,threading,trace,sys,sysconfig
from pathlib import Path
from codetracer_pure_python_recorder import cli
original=trace;original_main=trace.main;original_class=trace.Trace
assert Path(trace.__file__).resolve()==(Path(sysconfig.get_path("stdlib"))/"trace.py").resolve()
assert cli._reference is None and cli._REFERENCE_NAME not in sys.modules
barrier=threading.Barrier(4)
def load(_):
    barrier.wait();return cli._owned_reference()
with concurrent.futures.ThreadPoolExecutor(max_workers=4) as pool:
    modules=list(pool.map(load,range(4)))
assert all(module is modules[0] for module in modules)
assert Path(modules[0].__file__).resolve()==Path(cli.__file__).resolve().parent.parent/'trace.py'
assert all(cli._owned_reference() is modules[0] for _ in range(20))
assert sys.modules['trace'] is original and trace.main is original_main and trace.Trace is original_class
"""
        self.run_required([str(self.python), "-c", code])

    def test_explicit_arguments_ignore_unrelated_console_arguments(self):
        cwd = self.root / "explicit"
        cwd.mkdir()
        arguments = [str(self.program), "preserved-extra"]
        code = "from codetracer_pure_python_recorder import cli;import sys;sys.argv=['unrelated-console','wrong-program'];args=" + repr(arguments) + ";cli.main(args);assert args==" + repr(arguments)
        result = self.run_required([str(self.python), "-c", code], cwd=cwd)
        self.assertEqual(result.stdout, "42\n")
        self.assertEqual(json.loads((cwd / "trace_metadata.json").read_text()),
                         {"workdir": str(cwd), "program": "unrelated-console", "args": arguments})
        expected = self.record([str(self.python), str(self.reference)], "explicit-direct", str(self.reference))
        self.assertEqual(tuple(json.loads((cwd / name).read_text()) for name in ["trace.json", "trace_paths.json"]), expected)

    def test_original_empty_and_missing_program_errors(self):
        for name in ["codetracer-record", "codetracer-record-pure"]:
            script = str(self.python.parent / name)
            result = subprocess.run([script], env=self.environment, text=True, capture_output=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertEqual(result.stderr.strip(), "Usage: codetracer-record <program.py>")
            result = subprocess.run([script, str(self.root / "missing.py")], env=self.environment,
                                    text=True, capture_output=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("FileNotFoundError", result.stderr)

    def test_real_owned_module_failures_restore_registration(self):
        original = self.owned_trace.read_bytes()
        digest = hashlib.sha256(original).hexdigest()
        missing = self.owned_trace.with_suffix(".temporarily-absent")
        try:
            self.owned_trace.rename(missing)
            for prior in [False, True]:
                code = "import sys;from codetracer_pure_python_recorder import cli;previous=object();" + ("sys.modules[cli._REFERENCE_NAME]=previous;" if prior else "")
                code += "\ntry: cli._owned_reference()\nexcept ImportError as e: assert 'missing' in str(e);assert cli._reference is None;"
                code += ("assert sys.modules[cli._REFERENCE_NAME] is previous" if prior else "assert cli._REFERENCE_NAME not in sys.modules")
                code += "\nelse: raise AssertionError('missing artifact accepted')"
                self.run_required([str(self.python), "-c", code])
            missing.rename(self.owned_trace)
            for contents, error in [(b'raise RuntimeError("genuine-load-error")\n', "genuine-load-error"),
                                    (b'value = 42\n', "no callable main")]:
                self.owned_trace.write_bytes(contents)
                for prior in [False, True]:
                    code = "import sys;from codetracer_pure_python_recorder import cli;previous=object();" + ("sys.modules[cli._REFERENCE_NAME]=previous;" if prior else "")
                    code += "\ntry: cli._owned_reference()\nexcept Exception as e: assert " + repr(error) + " in str(e);assert cli._reference is None;"
                    code += ("assert sys.modules[cli._REFERENCE_NAME] is previous" if prior else "assert cli._REFERENCE_NAME not in sys.modules")
                    code += "\nelse: raise AssertionError('invalid real module accepted')"
                    self.run_required([str(self.python), "-c", code])
        finally:
            if missing.exists():
                missing.rename(self.owned_trace)
            self.owned_trace.write_bytes(original)
        self.assertEqual(hashlib.sha256(self.owned_trace.read_bytes()).hexdigest(), digest)
        self.run_required([str(self.python), "-c", "from codetracer_pure_python_recorder import cli;assert callable(cli._owned_reference().main)"])

    def test_real_wrong_module_fails_then_byte_restored_full_recording(self):
        original = self.owned_cli.read_bytes()
        digest = hashlib.sha256(original).hexdigest()
        cwd = self.root / "wrong-module"
        cwd.mkdir()
        script = str(self.python.parent / "codetracer-record")
        try:
            self.owned_cli.write_text("def main(argv=None):\n    from trace import main as foreign_main\n    return foreign_main(argv)\n")
            result = subprocess.run([script, str(self.program)], cwd=cwd, env=self.environment,
                                    text=True, capture_output=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("TypeError", result.stderr)
            self.assertFalse((cwd / "trace.json").exists())
        finally:
            self.owned_cli.write_bytes(original)
        self.assertEqual(hashlib.sha256(self.owned_cli.read_bytes()).hexdigest(), digest)
        expected = self.record([str(self.python), str(self.reference)], "wrong-module-direct", str(self.reference))
        self.assertEqual(self.record([script], "wrong-module-restored", script), expected)


if __name__ == "__main__":
    unittest.main()
