"""Exercise genuine Git registration and filesystem ownership, without mocks.

Each case owns a temporary primary/linked worktree populated with the actual
owning declaration bytes. It never changes the production checkout or a user
hook. Native installer/template/default-hook controls remain separate tests.
"""
from __future__ import annotations
import importlib.util
import hashlib
import os
import stat
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location("owned_hook_installer", ROOT / "tools/install-canonical-hooks.py")
if SPEC is None or SPEC.loader is None:
    raise RuntimeError("missing actual owning installer module")
INSTALLER = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(INSTALLER)


class CommonHooksOwnership(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory(prefix="python-hook-ownership-")
        self.addCleanup(self.temporary.cleanup)
        self.base = Path(self.temporary.name)
        self.primary = self.base / "primary"
        self.linked = self.base / "linked"
        self.primary.mkdir()
        self.git = shutil.which("git")
        if self.git is None:
            self.fail("missing declared native Git")
        self.command("init", "-q")
        self.command("config", "user.name", "Owned hook fixture")
        self.command("config", "user.email", "owned-hook-fixture@example.invalid")
        for name in INSTALLER.OWNING_DECLARATIONS:
            source = ROOT / name
            if source.is_symlink() or not source.is_file():
                self.fail(f"missing regular owning declaration: {name}")
            target = self.primary / name
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(source.read_bytes())
        self.command("add", "--", *INSTALLER.OWNING_DECLARATIONS)
        self.command("commit", "-q", "-m", "Own exact declaration fixture")
        self.command("worktree", "add", "--detach", str(self.linked), "HEAD")

    def command(self, *args: str) -> str:
        return subprocess.check_output([self.git, *args], cwd=self.primary, text=True, stderr=subprocess.STDOUT).strip()

    def inventory(self) -> dict[str, tuple[str, int, str]]:
        result = {}
        for path in sorted(self.primary.rglob("*")):
            mode = stat.S_IMODE(path.lstat().st_mode)
            if path.is_symlink():
                result[str(path.relative_to(self.primary))] = ("link", mode, os.readlink(path))
            elif path.is_file():
                result[str(path.relative_to(self.primary))] = ("file", mode, hashlib.sha256(path.read_bytes()).hexdigest())
            elif path.is_dir():
                result[str(path.relative_to(self.primary))] = ("directory", mode, "")
            else:
                self.fail(f"unexpected special fixture path: {path}")
        return result

    def assert_installer_refuses(self, expected: str, extra_env: dict[str, str] | None = None, bootstrap: bool = False) -> None:
        repro = os.environ.get("REPROBUILD_REPRO") or shutil.which("repro")
        if not repro:
            self.fail("missing declared matching native Repro for real installer refusal control")
        env = os.environ.copy()
        env["PYTHONDONTWRITEBYTECODE"] = "1"
        if extra_env:
            env.update(extra_env)
        before = self.inventory()
        result = subprocess.run(
            [os.sys.executable, str(self.primary / "tools/install-canonical-hooks.py"), "--repro", repro]
            + (["--bootstrap-managed"] if bootstrap else []),
            cwd=self.primary, env=env, capture_output=True, text=True,
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn(expected, result.stdout + result.stderr)
        self.assertEqual(self.inventory(), before, "refused installation changed owned filesystem bytes/types/modes")

    def test_missing_managed_bootstrap_refuses_before_writes(self) -> None:
        self.assert_installer_refuses("must be established by ownership-checked caller bootstrap")

    def test_real_fresh_matching_bootstrap(self) -> None:
        repro = os.environ.get("REPROBUILD_REPRO") or shutil.which("repro")
        if not repro:
            self.fail("missing declared matching engine")
        env = os.environ.copy()
        env["PYTHONDONTWRITEBYTECODE"] = "1"
        source_before = {name: (self.primary / name).read_bytes() for name in INSTALLER.OWNING_DECLARATIONS}
        result = subprocess.run(
            [os.sys.executable, str(self.primary / "tools/install-canonical-hooks.py"),
                "--bootstrap-managed", "--repro", repro], cwd=self.primary,
            env=env, capture_output=True, text=True,
        )
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("Ownership-checked matching managed bootstrap complete", result.stdout)
        hooks = self.primary / ".git/hooks"
        self.assertEqual(set(INSTALLER.pre_push_inventory(hooks)), {"pre-push", "pre-push.repro-managed", "pre-push.sample"})
        for name, expected in INSTALLER.KNOWN_MANAGED_BODIES.items():
            target = hooks / name
            self.assertFalse(target.is_symlink())
            self.assertEqual(stat.S_IMODE(target.stat().st_mode), 0o755)
            self.assertEqual(hashlib.sha256(target.read_bytes()).hexdigest(), expected)
        self.assertEqual({name: (self.primary / name).read_bytes() for name in INSTALLER.OWNING_DECLARATIONS}, source_before)

    def test_unknown_hook_refuses_before_bootstrap_writes(self) -> None:
        hook = self.primary / ".git/hooks/pre-commit"
        hook.write_bytes(b"#!/bin/sh\necho genuinely unknown owned fixture\n")
        hook.chmod(0o755)
        self.assert_installer_refuses("unknown or modified owning local hook body", bootstrap=True)

    def test_modified_native_sample_body_and_mode_refuse_before_bootstrap(self) -> None:
        sample = self.primary / ".git/hooks/pre-push.sample"
        original = sample.read_bytes()
        original_mode = stat.S_IMODE(sample.stat().st_mode)
        for mutation in ("body", "mode"):
            with self.subTest(mutation=mutation):
                try:
                    if mutation == "body":
                        sample.write_bytes(original + b"# genuinely modified sample\n")
                    else:
                        sample.chmod(0o644)
                    self.assert_installer_refuses("modified pre-push hook body or mode", bootstrap=True)
                finally:
                    sample.write_bytes(original)
                    sample.chmod(original_mode)
                self.assertEqual(sample.read_bytes(), original)
                self.assertEqual(stat.S_IMODE(sample.stat().st_mode), original_mode)

    def test_unknown_managed_body_refuses_before_writes(self) -> None:
        hook = self.primary / ".git/hooks/pre-commit"
        hook.write_bytes(b"#!/usr/bin/env sh\n# reprobuild hook dispatcher protocol=2\n# deliberate modified fixture\n")
        hook.chmod(0o755)
        self.assert_installer_refuses("unknown or modified managed hook template")

    def test_broken_existing_and_local_links_refuse_before_writes(self) -> None:
        for name in ("pre-commit", "pre-commit.repro-local"):
            with self.subTest(name=name):
                hook = self.primary / ".git/hooks" / name
                hook.symlink_to(self.base / "genuinely-absent-hook")
                try:
                    self.assert_installer_refuses("symlink")
                finally:
                    hook.unlink()

    def test_external_effective_global_path_refuses_before_writes(self) -> None:
        external = self.base / "external-hooks"
        external.mkdir()
        config = self.base / "private-global-gitconfig"
        subprocess.run([self.git, "config", "--file", str(config), "core.hooksPath", str(external)], check=True)
        original_config = config.read_bytes()
        self.assert_installer_refuses(
            "nonowning or unqualified hooks directory",
            {"GIT_CONFIG_GLOBAL": str(config), "GIT_CONFIG_NOSYSTEM": "1"},
        )
        self.assertEqual(config.read_bytes(), original_config)
        self.assertEqual(list(external.iterdir()), [])

    def test_symlink_common_hooks_refuses_before_writes(self) -> None:
        hooks = self.primary / ".git/hooks"
        backup = self.primary / ".git/hooks-fixture-backup"
        hooks.rename(backup)
        external = self.base / "external-hooks"
        external.mkdir()
        hooks.symlink_to(external, target_is_directory=True)
        self.assert_installer_refuses("own hooks directory is symlink")
        self.assertEqual(list(external.iterdir()), [])

    def test_primary_and_registered_linked_worktree(self) -> None:
        expected = self.primary / ".git"
        self.assertEqual(INSTALLER.owned_common_directory(self.git, self.primary), expected)
        self.assertEqual(INSTALLER.owned_common_directory(self.git, self.linked), expected)

    def test_different_config_refuses_without_changes(self) -> None:
        config = self.linked / ".pre-commit-config.yaml"
        original = config.read_bytes()
        config.write_bytes(original + b"\n# deliberately different owned fixture\n")
        before = self.command("config", "--local", "--list")
        with self.assertRaisesRegex(RuntimeError, "declaration differs"):
            INSTALLER.owned_common_directory(self.git, self.linked)
        self.assertEqual(self.command("config", "--local", "--list"), before)
        self.assertEqual(config.read_bytes(), original + b"\n# deliberately different owned fixture\n")

    def test_different_sdk_refuses(self) -> None:
        sdk = self.linked / "tools/python-sdk/default.nix"
        sdk.write_bytes(sdk.read_bytes() + b"\n# deliberately different SDK fixture\n")
        with self.assertRaisesRegex(RuntimeError, "declaration differs"):
            INSTALLER.owned_common_directory(self.git, self.linked)

    def test_unregistered_link_refuses(self) -> None:
        unregistered = self.base / "unregistered"
        unregistered.mkdir()
        (unregistered / ".git").write_bytes((self.linked / ".git").read_bytes())
        with self.assertRaisesRegex(RuntimeError, "not registered"):
            INSTALLER.owned_common_directory(self.git, unregistered)

    def test_symlink_declaration_refuses(self) -> None:
        config = self.linked / ".pre-commit-config.yaml"
        config.unlink()
        config.symlink_to(self.primary / ".pre-commit-config.yaml")
        with self.assertRaisesRegex(RuntimeError, "not regular"):
            INSTALLER.owned_common_directory(self.git, self.linked)


    def test_configured_owner_native_generation_preserves_config(self) -> None:
        hooks = self.primary / ".git/hooks"
        self.command("config", "core.hooksPath", str(hooks))
        original = (self.primary / ".git/config").read_bytes()
        env = INSTALLER.native_install_environment(os.environ.copy(), self.git, self.primary, hooks)
        prek = shutil.which("prek")
        self.assertIsNotNone(prek, "missing declared native Prek")
        subprocess.run([prek, "install"], cwd=self.primary, env=env, check=True)
        generated = hooks / "pre-commit"
        self.assertEqual(generated.read_bytes(), INSTALLER.native_generated_body(prek))
        self.assertEqual(stat.S_IMODE(generated.stat().st_mode), 0o755)
        self.assertEqual((self.primary / ".git/config").read_bytes(), original)
        self.assertEqual(self.command("config", "--get", "core.hooksPath"), str(hooks))

    def test_ambiguous_command_configuration_refuses_without_writes(self) -> None:
        original = self.inventory()
        for key in ("GIT_CONFIG_COUNT", "GIT_CONFIG_PARAMETERS", "GIT_CONFIG_KEY_0", "GIT_CONFIG_VALUE_0"):
            env = os.environ.copy()
            env[key] = "1"
            with self.assertRaisesRegex(RuntimeError, "ambiguous inherited"):
                INSTALLER.native_install_environment(env, self.git, self.primary, self.primary / ".git/hooks")
            self.assertEqual(self.inventory(), original)

if __name__ == "__main__":
    unittest.main()
