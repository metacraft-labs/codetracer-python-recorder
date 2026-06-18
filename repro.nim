## Reprobuild dev env + build recipe for codetracer-python-recorder.
##
## Ships a Python package whose native extension is implemented in
## Rust + PyO3 and built through ``maturin``. The recipe expresses
## the cargo build + cargo test edges natively per
## ``codetracer-specs/Repo-Requirements.md`` §2.8 via the reprobuild
## stdlib's typed ``cargo.build`` / ``cargo.test`` wrappers — no
## ``shell(command = "bash ...")`` indirections around cargo itself.
## The PyO3 build script's ``PYO3_PYTHON`` env var is injected via
## the typed wrappers' ``extraEnv`` parameter (MR10) so the recipe
## stays free of shell-quoting concerns and the engine sees a clean
## ``cargo build``/``cargo test`` argv it can deduplicate and
## fingerprint directly.
##
## ## M7 (Windows reprobuild migration) provisioning posture
##
## The recipe uses reprobuild's stdlib tarball provisioning for the
## Python toolchain — replacing the env.ps1 ensure-script set:
##
##   - ``python-dev``  → astral-sh/python-build-standalone tarball
##                       (carries ``libs/python312.lib`` + ``include/``;
##                       see ``libs/repro_dsl_stdlib/.../python_dev.nim``)
##   - ``uv``          → astral-sh/uv standalone zip
##                       (carries ``uv.exe`` + ``uvx.exe``)
##   - ``maturin``     → bootstrapped via ``uv tool install``
##   - ``pytest``      → bootstrapped via ``uv tool install``
##
## maturin + pytest are Python packages; they have no standalone
## binary distribution. Rather than adding fragile per-tool tarball
## pins (see the M7 header notes in ``packages/maturin.nim`` +
## ``packages/pytest.nim``), the recipe materialises them via a
## ``uv tool install`` bootstrap edge against a workspace-local
## ``.repro/uv-tools/bin/`` directory. That bootstrap edge is the
## sole remaining ``shell()`` invocation in this recipe; the
## downstream cargo edges do NOT need ``.repro/uv-tools/bin`` on
## PATH because cargo never invokes maturin or pytest during
## ``cargo build`` / ``cargo test`` (those tools only run during
## wheel packaging + Python-side pytest, which the Justfile drives
## outside this recipe).
##
## ## Phase 3 migration off declaredOnly / bash wrappers
##
## Earlier revisions (MR7/MR12) wrapped the three cargo invocations
## below in ``bash -c '...'`` to inject ``PYO3_PYTHON`` + a
## ``.repro/uv-tools/bin`` PATH prefix, and opted those bash edges
## out of the automatic-monitor IAT-patching shim via
## ``dependencyPolicy = declaredOnlyDependencyPolicy()`` to avoid
## the rustc-on-Windows STATUS_ACCESS_VIOLATION crash class that
## monitor-shim hooks induced. Reprobuild MR16 removed
## ``declaredOnlyDependencyPolicy`` entirely — the typed
## ``cargo.build`` / ``cargo.test`` wrappers now collect dependency
## evidence from cargo's own ``target/<profile>/deps/*.d`` make-format
## depfiles (``dependencyPolicy makeDepfile`` in
## ``packages/cargo.nim``). That gathering kind does NOT inject the
## monitor shim into rustc / link.exe, so the original crash class is
## still avoided while the engine gets first-class dependency tracking.
## This recipe accordingly switches all three cargo edges to typed
## wrappers and passes ``PYO3_PYTHON`` via ``extraEnv``.
##
## On Linux/macOS the nix flake supplies all required tools; the
## recipe stays platform-uniform under the engine's default
## provisioning (Nix on Nix-capable hosts, tarball on Windows /
## non-Nix Linux). MR2 dropped the legacy
## ``defaultToolProvisioning "path"`` declaration now that every tool
## the recipe consumes has a stdlib tarball entry.

import repro_project_dsl
import repro_dsl_stdlib/packages/sh

package codetracer_python_recorder:
  uses:
    "rustc >=1.85"
    "cargo >=1.85"
    "python-dev >=3.10"
    "nim >=2.2 <3.0"
    "nimble"
    "capnp"
    "zstd"
    "uv"
    "sh"
    when not defined(windows):
      # pkg-config + OpenSSL — only the unix build path consumes
      # openssl-sys; the Windows build uses rustls instead. maturin /
      # pytest stay on the Nix path here (the flake provides them as
      # nixpkgs#maturin / nixpkgs#python3Packages.pytest); Windows
      # bootstraps them through the uv-tool-install edge below.
      "pkg-config"
      "openssl"
      "maturin"
      "pytest"

  library codetracerPythonRecorder

  devEnv:
    activity "default"

  build:
    # ---- Bootstrap maturin + pytest via uv tool install --------------
    #
    # Workspace-local tool dir lives at
    # ``.repro/uv-tools/{bin,tools}/``. ``uv tool install`` drops
    # console-script shims under ``bin/`` (``maturin.exe`` +
    # ``pytest.exe`` on Windows; bare names on Linux/macOS) and the
    # tool environments themselves under ``tools/``. The two env vars
    # ``UV_TOOL_BIN_DIR`` / ``UV_TOOL_DIR`` redirect ``uv`` away from
    # its default ``%LOCALAPPDATA%\uv\tools`` (which would leak the
    # install outside the workspace and break action-cache
    # reproducibility).
    #
    # The ``--python <name>`` argument pins the interpreter to the
    # python-dev install the engine put on PATH. ``uv`` and PyO3 both
    # accept a bare interpreter name (``python`` / ``python3`` /
    # ``python.exe``) and resolve it against PATH themselves — so we
    # spell the bare name here rather than substituting via
    # ``$(command -v ...)``. The substitution form was incompatible
    # with the Windows action-runner: on Windows the shell action's
    # outer process is cmd.exe (sh.exe is invoked per-line through the
    # ``shell()`` wrapper but the surrounding ``$(...)`` evaluation
    # never reaches sh.exe — cmd.exe sees a literal ``command`` token
    # and bails with ``'command' is not recognized``). Bare names
    # avoid the issue and work uniformly across Nix-shell bash (PATH
    # holds the flake-provided interpreter), Windows cmd.exe (PATH
    # holds the python-build-standalone interpreter the engine
    # materialised), and the wrapped ``bash -c`` script body below.
    # The action body itself is bash-only (``set -euo pipefail``,
    # ``mkdir -p``, ``&&`` chaining, ``$(pwd)``), so we wrap the whole
    # body in ``bash -c '...'``: even when the outer dispatcher is
    # cmd.exe (Windows), ``bash -c`` invokes ``bash.exe`` directly
    # from the ``sh`` / ``bash`` tool prefix already on PATH via
    # ``uses: "sh"`` (PortableGit ships both ``sh.exe`` and
    # ``bash.exe`` in the same prefix). On Linux/macOS the outer
    # ``sh -c`` is already bash-compatible so the nested ``bash -c``
    # is a no-op overhead.
    const uvToolBinDir = ".repro/uv-tools/bin"
    const uvToolDir = ".repro/uv-tools/tools"
    const uvToolBootstrapMarker = ".repro/uv-tools/.bootstrap-ok"
    const pythonInterpreter =
      when defined(windows): "python.exe" else: "python3"

    let uvToolBootstrapBody = (
      "set -euo pipefail && " &
      "mkdir -p " & uvToolBinDir & " " & uvToolDir & " && " &
      "UV_TOOL_BIN_DIR=\"$(pwd)/" & uvToolBinDir & "\" " &
      "UV_TOOL_DIR=\"$(pwd)/" & uvToolDir & "\" " &
      "uv tool install --python " & pythonInterpreter & " maturin && " &
      "UV_TOOL_BIN_DIR=\"$(pwd)/" & uvToolBinDir & "\" " &
      "UV_TOOL_DIR=\"$(pwd)/" & uvToolDir & "\" " &
      "uv tool install --python " & pythonInterpreter & " pytest && " &
      "date -u +%FT%TZ > " & uvToolBootstrapMarker)

    let uvToolBootstrap = shell(
      command = "bash -c '" & uvToolBootstrapBody & "'",
      actionId = "codetracer-python-recorder.uv-tool-bootstrap",
      extraOutputs = @[uvToolBootstrapMarker])

    # ---- Native cargo build of the PyO3 extension --------------------
    #
    # The cargo crate ``codetracer-python-recorder`` produces a cdylib
    # (.pyd on Windows, .so on Linux, .dylib on macOS) which the
    # Python package imports as ``codetracer_python_recorder``.
    #
    # PyO3 links against ``python<ver>.lib`` from the Python install's
    # ``libs/`` directory. ``PYO3_PYTHON`` tells the PyO3 build script
    # which interpreter to inspect for the link target; we pass the
    # bare interpreter name (``python.exe`` on Windows, ``python3``
    # on Linux/macOS) via ``extraEnv`` and the engine's
    # ``actionPathPrefix`` (see ``toolPathPrefix`` in
    # ``repro_cli_support``) makes the python-dev install's
    # ``python.exe`` first-on-PATH for the cargo action, so PyO3's
    # ``PATH``-lookup resolves to the tarball-realized interpreter.
    #
    # ``.repro/uv-tools/bin`` is NOT prepended to PATH for the cargo
    # edges: cargo build / cargo test never invoke maturin or pytest
    # (those tools only run during wheel packaging + Python-side
    # pytest, which the Justfile drives outside this recipe). The
    # ``uv-tool-bootstrap`` edge above remains a hard dependency only
    # because downstream ``just test`` invocations need
    # ``maturin.exe`` + ``pytest.exe`` on PATH at runtime.
    #
    # cargo emits a cdylib under cargo's native naming: ``.dll`` on
    # Windows (PyO3+maturin only renames it to ``.pyd`` at wheel-build
    # time), ``.dylib`` on macOS, ``.so`` on Linux. The recipe's
    # ``extraOutputs`` marker MUST match the bytes cargo actually
    # writes so the engine's freshness check observes the artifact.
    const dylibExt =
      when defined(windows): "dll"
      elif defined(macosx): "dylib"
      else: "so"
    const extensionBinary =
      "codetracer-python-recorder/target/release/codetracer_python_recorder." &
      dylibExt

    let pyo3Env = @[("PYO3_PYTHON", pythonInterpreter)]

    let extensionBuild = cargo.build(
      release = true,
      locked = true,
      manifestPath = "codetracer-python-recorder/Cargo.toml",
      actionId = "codetracer-python-recorder.cargo-build",
      after = @[uvToolBootstrap],
      extraInputs = @[
        "codetracer-python-recorder/Cargo.toml",
        "codetracer-python-recorder/Cargo.lock",
        "codetracer-python-recorder/src",
        "codetracer-python-recorder/crates",
        uvToolBootstrapMarker
      ],
      extraOutputs = @[extensionBinary],
      extraEnv = pyo3Env)
    discard collect("default", @[extensionBuild])

    # ---- Rust-side cargo tests ---------------------------------------
    #
    # Two edges: ``cargo test --no-run`` builds every test binary
    # under ``target/debug/deps/<crate>-<hash>``; the second
    # ``cargo test`` (without ``--no-run``) re-uses those binaries and
    # actually runs them. The dependency from run -> build runs through
    # ``after`` plus the deps-dir entry in ``extraInputs``.
    let cargoTestsBuild = cargo.test(
      noRun = true,
      locked = true,
      manifestPath = "codetracer-python-recorder/Cargo.toml",
      actionId = "codetracer-python-recorder.cargo-test-build",
      after = @[uvToolBootstrap],
      extraInputs = @[
        "codetracer-python-recorder/Cargo.toml",
        "codetracer-python-recorder/Cargo.lock",
        "codetracer-python-recorder/src",
        "codetracer-python-recorder/tests",
        uvToolBootstrapMarker
      ],
      extraOutputs = @["codetracer-python-recorder/target/debug/deps"],
      extraEnv = pyo3Env)

    let cargoTestsRun = cargo.test(
      locked = true,
      manifestPath = "codetracer-python-recorder/Cargo.toml",
      actionId = "codetracer-python-recorder.cargo-test-run",
      after = @[cargoTestsBuild.action],
      extraInputs = @[
        "codetracer-python-recorder/Cargo.toml",
        "codetracer-python-recorder/Cargo.lock",
        "codetracer-python-recorder/src",
        "codetracer-python-recorder/tests",
        "codetracer-python-recorder/target/debug/deps",
        uvToolBootstrapMarker
      ],
      extraEnv = pyo3Env)

    # Python-side pytest is driven through the Justfile + uv
    # workflow for now; wiring it as a reprobuild edge requires a
    # typed-tool subcmd on pytest.nim (the stdlib package only
    # declares provisioning today). Until that lands the cargo
    # test edge covers the rust-side test set; ``just test``
    # continues to drive the pytest suite via ``uv run --group
    # dev --group test pytest …`` per Justfile. The uv-tool-install
    # bootstrap above produces a ``pytest.exe`` shim under
    # ``.repro/uv-tools/bin`` that downstream wrappers can consume.
    discard collect("test", @[cargoTestsRun.action])
