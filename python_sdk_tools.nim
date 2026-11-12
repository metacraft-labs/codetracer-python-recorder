## The three tools of this repository's build SDK: the Python 3.12
## interpreter (with the setuptools and wheel build backend), uv and maturin.
##
## On Linux and macOS each one is a view of the single Nix SDK that
## ``tools/python-sdk/default.nix`` builds from this repository's own
## ``flake.lock`` and ``.python-version``. Its identity is the bytes of those
## files, so editing any of them selects a new SDK.
##
## Windows has no Nix, so each tool also has a ``tarball`` realisation from
## its upstream release archive, pinned by sha256, at the version the Nix
## SDK provides at this ``flake.lock`` (CPython 3.12.13, uv 0.9.30, maturin
## 1.9.6), so every platform builds with the same tool versions.
## ``repro.nim`` selects ``tarball`` provisioning on Windows. Each archive
## ships a native build for both x86_64 and aarch64 Windows, so neither arm
## runs under emulation.
##
## * CPython: python.org publishes no Windows binaries for 3.12 security
##   releases after 3.12.10, so the interpreter is the astral-sh
##   python-build-standalone ``install_only`` build, the portable CPython uv
##   itself installs. It is a full install (``python/python.exe`` with
##   ``include/``, ``libs/python312.lib`` and ``Lib/``), which PyO3 needs to
##   link the extension. The embeddable zip has neither the headers nor the
##   import library.
## * uv: astral-sh/uv's release zip, with ``uv.exe`` at the archive root.
## * maturin: PyO3/maturin's release zip, with ``maturin.exe`` at the archive
##   root.
##
## These arms provision the tools only. The graph's helper frontends in
## ``tools/`` still validate the SDK through the Nix SDK's identity marker
## (``RECORDER_TEST_BUILD_SDK`` and ``codetracer-python-sdk-identity.json``
## under ``/nix/store``), and the python-build-standalone archive ships pip
## without the setuptools and wheel build backend the Nix SDK's interpreter
## carries, so the Windows graph is not yet qualified.
import blake3
import repro_project_dsl

const sdkSourceBytes = staticRead("tools/python-sdk/default.nix") & "\0" &
  staticRead("flake.lock") & "\0" & staticRead("nix/python.nix") & "\0" &
  staticRead(".python-version")
let sdkSourceIdentity = blake3.toHex(blake3.digest(sdkSourceBytes))

package `python-recorder-python-sdk`:
  provisioning:
    nixPackage "codetracer-python-recorder-build-sdk", executablePath = "bin/python",
      expressionFile = "tools/python-sdk/default.nix",
      lockIdentity = "owning-python3.12+setuptools+wheel:" & sdkSourceIdentity
    # https://github.com/astral-sh/python-build-standalone/releases/tag/20260610
    tarball url = "https://github.com/astral-sh/python-build-standalone/releases/download/20260610/cpython-3.12.13+20260610-x86_64-pc-windows-msvc-install_only.tar.gz",
      sha256 = "f5e4d9f856567493776f3d1e832c939fbaba5dcbcc5e0492a82ecfceea83b316",
      archiveType = "tar.gz",
      executablePath = "python/python.exe",
      version = "3.12.13",
      packageId = "python-recorder-python-sdk@3.12.13+20260610",
      cpu = "x86_64",
      os = "windows",
      lockIdentity = "tarball:python-recorder-python-sdk@3.12.13+20260610:windows-x86_64:sha256:f5e4d9f856567493776f3d1e832c939fbaba5dcbcc5e0492a82ecfceea83b316"
    tarball url = "https://github.com/astral-sh/python-build-standalone/releases/download/20260610/cpython-3.12.13+20260610-aarch64-pc-windows-msvc-install_only.tar.gz",
      sha256 = "b50d4eee0a9c440597fb57a7b6b8f0021e799ebbf84d9fcb81f7ca199785e865",
      archiveType = "tar.gz",
      executablePath = "python/python.exe",
      version = "3.12.13",
      packageId = "python-recorder-python-sdk@3.12.13+20260610",
      cpu = "aarch64",
      os = "windows",
      lockIdentity = "tarball:python-recorder-python-sdk@3.12.13+20260610:windows-aarch64:sha256:b50d4eee0a9c440597fb57a7b6b8f0021e799ebbf84d9fcb81f7ca199785e865"
package `python-recorder-uv-sdk`:
  provisioning:
    nixPackage "codetracer-python-recorder-build-sdk", executablePath = "bin/uv",
      expressionFile = "tools/python-sdk/default.nix",
      lockIdentity = "owning-python-native-uv:" & sdkSourceIdentity
    # https://github.com/astral-sh/uv/releases/tag/0.9.30
    tarball url = "https://github.com/astral-sh/uv/releases/download/0.9.30/uv-x86_64-pc-windows-msvc.zip",
      sha256 = "875981be7908295937dee09532bb66d576986d4f223259e171b0c767c885897a",
      archiveType = "zip",
      executablePath = "uv.exe",
      version = "0.9.30",
      packageId = "python-recorder-uv-sdk@0.9.30",
      cpu = "x86_64",
      os = "windows",
      lockIdentity = "tarball:python-recorder-uv-sdk@0.9.30:windows-x86_64:sha256:875981be7908295937dee09532bb66d576986d4f223259e171b0c767c885897a"
    tarball url = "https://github.com/astral-sh/uv/releases/download/0.9.30/uv-aarch64-pc-windows-msvc.zip",
      sha256 = "cfbc40baf1da11c55eff92ee008f5af3cdbb4c24c40ddb0bbd489b983fadf43f",
      archiveType = "zip",
      executablePath = "uv.exe",
      version = "0.9.30",
      packageId = "python-recorder-uv-sdk@0.9.30",
      cpu = "aarch64",
      os = "windows",
      lockIdentity = "tarball:python-recorder-uv-sdk@0.9.30:windows-aarch64:sha256:cfbc40baf1da11c55eff92ee008f5af3cdbb4c24c40ddb0bbd489b983fadf43f"
package `python-recorder-maturin-sdk`:
  provisioning:
    nixPackage "codetracer-python-recorder-build-sdk", executablePath = "bin/maturin",
      expressionFile = "tools/python-sdk/default.nix",
      lockIdentity = "owning-python-native-maturin:" & sdkSourceIdentity
    # https://github.com/PyO3/maturin/releases/tag/v1.9.6
    tarball url = "https://github.com/PyO3/maturin/releases/download/v1.9.6/maturin-x86_64-pc-windows-msvc.zip",
      sha256 = "098e0dc3b2ebafe6e778ed913c31c5b464a3414d5a8e2f1db7264b9582f070e9",
      archiveType = "zip",
      executablePath = "maturin.exe",
      version = "1.9.6",
      packageId = "python-recorder-maturin-sdk@1.9.6",
      cpu = "x86_64",
      os = "windows",
      lockIdentity = "tarball:python-recorder-maturin-sdk@1.9.6:windows-x86_64:sha256:098e0dc3b2ebafe6e778ed913c31c5b464a3414d5a8e2f1db7264b9582f070e9"
    tarball url = "https://github.com/PyO3/maturin/releases/download/v1.9.6/maturin-aarch64-pc-windows-msvc.zip",
      sha256 = "638259f6e75f38ba245a55ef50f679cb3f7f873d8b4ece5d6908295a4ed69a66",
      archiveType = "zip",
      executablePath = "maturin.exe",
      version = "1.9.6",
      packageId = "python-recorder-maturin-sdk@1.9.6",
      cpu = "aarch64",
      os = "windows",
      lockIdentity = "tarball:python-recorder-maturin-sdk@1.9.6:windows-aarch64:sha256:638259f6e75f38ba245a55ef50f679cb3f7f873d8b4ece5d6908295a4ed69a66"
