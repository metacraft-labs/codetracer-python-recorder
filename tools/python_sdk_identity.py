"""Identity of the owning Python SDK, read from the record the realized tool provides.

The graph's native helper frontends (source identity, external cache
provisioning and runtime assembly) each run under the owning SDK interpreter
and must refuse to run under any other. This module is the one place that
decides whether they do.

The SDK's identity is the pinned source its provisioning mode actually
realized, and it is read from the identity record that realization carries:

* ``nix-flake-lock`` (Linux and macOS). ``tools/python-sdk/default.nix``
    builds the SDK from the nixpkgs node the committed ``flake.lock`` selects.
    It writes ``codetracer-python-sdk-identity.json`` at the SDK root, naming
    that node, and its ``python``/``uv``/``maturin`` wrappers export the root as
    ``RECORDER_TEST_BUILD_SDK`` and the source digest as
    ``RECORDER_TEST_BUILD_SDK_SOURCE_ID``.
* ``tarball`` (Windows). The identity is the archive URL and sha256 pinned in
    ``python_sdk_tools.nim``. Reprobuild does not yet give an action an
    identity record for a tarball-realized tool, so this case fails with
    :class:`CapabilityUnavailable`. It is never inferred.

An identity is never inferred from a path: where the SDK sits on disk says
nothing about which source it was built from. A record that is missing,
names an unknown provisioning kind, or does not match the committed sources
is refused.
"""

import hashlib
import importlib.metadata
import json
import os
import pathlib
import platform
import subprocess
import sys

# The committed files whose bytes identify the Nix SDK. tools/python-sdk/default.nix
# hashes the same list, in this order, into the record's ``sourceId``.
SOURCE_FILES = ["tools/python-sdk/default.nix", "flake.lock", "nix/python.nix", ".python-version"]
RECORD_NAME = "codetracer-python-sdk-identity.json"
NIX_FLAKE_LOCK = "nix-flake-lock"


class CapabilityUnavailable(RuntimeError):
    """The platform's SDK realization provides no identity record an action can read."""


def committed_source_id(root):
    """sha256 of the committed SDK source bytes, encoded as default.nix encodes them."""
    encoded = json.dumps(
        [(root / p).read_text() for p in SOURCE_FILES], ensure_ascii=False, separators=(",", ":")
    ).encode()
    return hashlib.sha256(encoded).hexdigest()


def committed_nixpkgs_selection(root):
    """The nixpkgs node the committed flake.lock selects for this repository."""
    lock = json.loads((root / "flake.lock").read_text())
    key = lock["nodes"][lock["root"]]["inputs"]["nixpkgs"]
    if not isinstance(key, str):
        raise RuntimeError("flake.lock's nixpkgs input is not a direct node reference")
    return key, lock["nodes"][key]["locked"]


def read_record(root):
    """Read and validate the realized SDK's identity record against the committed sources.

    Returns ``(sdk_root, record)``. Raises :class:`CapabilityUnavailable` on a
    platform whose realization provides no record, and ``RuntimeError`` when the
    record is missing, of an unknown kind, or does not match.
    """
    location = os.environ.get("RECORDER_TEST_BUILD_SDK", "")
    if not location:
        if os.name == "nt":
            raise CapabilityUnavailable(
                "capability unavailable: the tarball-realized python-recorder-python-sdk provides no "
                "identity record (its pinned archive URL and sha256) that an action can read; "
                "reprobuild per-tool provisioning overrides are required and are not in any release"
            )
        raise RuntimeError("the realized SDK provides no identity record: RECORDER_TEST_BUILD_SDK is unset")
    sdk = pathlib.Path(location)
    record_path = sdk / RECORD_NAME
    if not record_path.is_file():
        raise RuntimeError(f"the realized SDK has no identity record at {record_path}")
    record = json.loads(record_path.read_text())
    provisioning = record.get("provisioning")
    if not isinstance(provisioning, dict) or provisioning.get("kind") != NIX_FLAKE_LOCK:
        raise RuntimeError(f"SDK identity record names an unsupported provisioning kind: {provisioning!r}")
    key, locked = committed_nixpkgs_selection(root)
    if provisioning.get("flakeLockInput") != key or provisioning.get("locked") != locked:
        raise RuntimeError("SDK identity record does not name the committed flake.lock nixpkgs selection")
    expected = committed_source_id(root)
    if record.get("sourceId") != expected or os.environ.get("RECORDER_TEST_BUILD_SDK_SOURCE_ID") != expected:
        raise RuntimeError("SDK source identity mismatch")
    for field in ["underlyingPython", "underlyingUv", "underlyingMaturin"]:
        if not pathlib.Path(record[field]).is_file():
            raise RuntimeError(f"SDK identity record names a missing {field}: {record[field]}")
    return sdk, record


def owning_sdk(root):
    """Validate that this process runs under the owning SDK; return ``(sdk_root, record)``.

    The returned record also carries the setuptools and wheel versions and
    module paths measured in this interpreter.
    """
    sdk, record = read_record(root)
    if pathlib.Path(sys.executable).resolve() != pathlib.Path(record["underlyingPython"]).resolve():
        raise RuntimeError("frontend is not executing the owning SDK interpreter")
    if sys.version_info[:2] != (3, 12) or platform.python_version() != record["pythonVersion"]:
        raise RuntimeError("frontend ABI differs from the approved Python3.12 SDK")
    record["setuptoolsActual"] = importlib.metadata.version("setuptools")
    record["wheelActual"] = importlib.metadata.version("wheel")
    import setuptools
    import wheel

    record["setuptoolsModule"] = setuptools.__file__
    record["wheelModule"] = wheel.__file__
    # The build backend must come from the SDK interpreter's own package
    # closure, not from whatever else is importable.
    backend_root = pathlib.Path(record["underlyingPython"]).parent.parent
    for field, package in [("setuptoolsModule", "setuptools"), ("wheelModule", "wheel")]:
        actual_module = pathlib.Path(record[field]).resolve(strict=True)
        selected_module = backend_root / "lib" / "python3.12" / "site-packages" / package / "__init__.py"
        if actual_module != selected_module.resolve(strict=True):
            raise RuntimeError("backend module differs from the selected SDK package: " + package)
    actual = subprocess.check_output([str(sdk / "bin/uv"), "--version"], text=True).split()
    if actual[:2] != ["uv", record["uvVersion"]]:
        raise RuntimeError("UV frontend differs from the SDK identity record")
    return sdk, record
