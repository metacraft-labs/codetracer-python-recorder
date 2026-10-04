## Complete recorder graph with approved local component qualification.
## Measured primary baseline7410f415ece99125d820ae207b9d6716f20210f7 plus
## reviewed changes, frozen13/f39/Nim7e: canonical native160/pytest178,
## default3/test10/bench9, lint and shipping passed in terminal receipt
## python-approved-bc13-complete-gates-attempt-2.json. Bench9 includes
## eight launched actions and one cached ctPrint; no overall landing claim.
## Real compiler and test boundaries only; native fetch cachefalse retains
## documented immutable766 network-monitor loss rather than bypassing it.
import std/os
import repro_project_dsl
import python_sdk_tools
import "../codetracer-trace-format-nim/build_writer_artifacts"

package codetracer_python_recorder:
  uses:
    "python-recorder-python-sdk"
    "python-recorder-uv-sdk"
    "python-recorder-maturin-sdk"
    "rustc >=1.85"
    "cargo >=1.85"
    "cargo-nextest"
    "nim >=2.2 <3.0"
    "nimble"
    "git"
    "capnp"
    "zstd"
    "bash"
    "dirname"
    "grep"
    when defined(linux):
      "gcc"
      "pkg-config"
      "openssl"
    elif defined(macosx):
      "clang"
      "pkg-config"
      "openssl"
  library codetracerPythonRecorder
  build:
    let base = ".repro/build/python-complete-graph"
    let root = activeProviderProjectRoot()
    let sdkInputs = @["tools/python-sdk/default.nix", "flake.lock", "nix/python.nix", ".python-version", "python_sdk_tools.nim", "repro.nim"]
    var nativeRefs = @["python-recorder-python-sdk", "python-recorder-maturin-sdk", "cargo", "rustc", "nim", "nimble", "git", "capnp", "zstd"]
    when defined(linux): nativeRefs.add(@["gcc", "pkg-config", "openssl"])
    elif defined(macosx): nativeRefs.add(@["clang", "pkg-config", "openssl"])
    let archive = base / "pure/source.tar.gz"
    let archiveManifest = base / "pure/source-manifest.json"
    let stageCall = publicCliCall("python-recorder-python-sdk", "python-recorder-python-sdk", "", "python.pure-stage", @[
      cliArgSeq("args", @["tools/python_pure_source_archive.py", "--root", root, "--archive", archive, "--manifest", archiveManifest], kind = cpkPositional)])
    let stage = buildAction("python.pure-stage", stageCall,
      inputs = sdkInputs & @["tools/python_pure_source_archive.py", "codetracer-pure-python-recorder", ".git/index"],
      outputs = @[archive, archiveManifest], declaredOutputs = @[archive, archiveManifest],
      toolIdentityRefs = @["python-recorder-python-sdk", "git"], dependencyPolicy = automaticMonitorPolicy(), cacheable = false)
    let pureCall = publicCliCall("python-recorder-uv-sdk", "python-recorder-uv-sdk", "", "python.pure-wheel", @[
      cliArgSeq("args", @["build", archive, "--wheel", "--out-dir", base / "pure/wheels", "--python", "python", "--no-build-isolation", "--no-managed-python", "--offline"], kind = cpkPositional)])
    let pureWheel = buildAction("python.pure-wheel", pureCall, deps = @[stage.id],
      inputs = sdkInputs & @[archive, archiveManifest], outputs = @[base / "pure/wheels"], declaredOutputs = @[base / "pure/wheels"],
      scratchDirs = @[base / "pure/cache"], env = @[("UV_CACHE_DIR", root / base / "pure/cache")],
      toolIdentityRefs = @["python-recorder-uv-sdk", "python-recorder-python-sdk"], dependencyPolicy = automaticMonitorPolicy(), cacheable = false)
    let nativeInputs = sdkInputs & @["codetracer-python-recorder/Cargo.toml", "codetracer-python-recorder/Cargo.lock", "codetracer-python-recorder/pyproject.toml", "codetracer-python-recorder/README.md", "codetracer-python-recorder/LICENSE", "codetracer-python-recorder/src", "codetracer-python-recorder/codetracer_python_recorder", "../codetracer-trace-format/Cargo.toml", "../codetracer-trace-format/Cargo.lock", "../codetracer-trace-format/*/Cargo.toml", "../codetracer-trace-format/*/build.rs", "../codetracer-trace-format/*/src", "../codetracer-trace-format-nim/src", "../codetracer-trace-format-nim/build_ffi.nims", "../codetracer-trace-format-nim/build_ffi_flags.nim", "../codetracer-trace-format-nim/codetracer_trace_format.nimble"]
    let nativeCall = publicCliCall("python-recorder-maturin-sdk", "python-recorder-maturin-sdk", "", "python.native-integration-wheel", @[
      cliArgSeq("args", @["build", "--locked", "--manifest-path", "codetracer-python-recorder/Cargo.toml", "--features", "integration-test", "--interpreter", "python", "--out", base / "native-test/wheels"], kind = cpkPositional)])
    let nativeWheel = buildAction("python.native-integration-wheel", nativeCall,
      inputs = nativeInputs, outputs = @[base / "native-test/wheels"], declaredOutputs = @[base / "native-test/wheels"],
      scratchDirs = @[base / "native-test/cargo"], env = @[("CARGO_TARGET_DIR", root / base / "native-test/cargo"), ("CARGO_BUILD_JOBS", "2")],
      toolIdentityRefs = nativeRefs, dependencyPolicy = automaticMonitorPolicy(), cacheable = false)
    let shippingCall = publicCliCall("python-recorder-maturin-sdk", "python-recorder-maturin-sdk", "", "python.shipping-wheel-sdist", @[
      cliArgSeq("args", @["build", "--locked", "--manifest-path", "codetracer-python-recorder/Cargo.toml", "--release", "--sdist", "--interpreter", "python", "--out", base / "shipping/artifacts"], kind = cpkPositional)])
    let shipping = buildAction("python.shipping-wheel-sdist", shippingCall,
      inputs = nativeInputs & @["codetracer-python-recorder/tests"],
      outputs = @[base / "shipping/artifacts"], declaredOutputs = @[base / "shipping/artifacts"],
      scratchDirs = @[base / "shipping/cargo"], env = @[("CARGO_TARGET_DIR", root / base / "shipping/cargo"), ("CARGO_BUILD_JOBS", "2")],
      toolIdentityRefs = nativeRefs, dependencyPolicy = automaticMonitorPolicy(), cacheable = false)
    let sourceManifest = base / "source/identity.json"
    let identityCall = publicCliCall("python-recorder-python-sdk", "python-recorder-python-sdk", "", "python.source-identity", @[
      cliArgSeq("args", @["tools/python_source_identity.py", "--root", root, "--manifest", sourceManifest], kind = cpkPositional)])
    let sourceIdentity = buildAction("python.source-identity", identityCall,
      inputs = sdkInputs & @["uv.lock", "pyproject.toml", "codetracer-python-recorder/pyproject.toml", "codetracer-python-recorder/Cargo.toml", "codetracer-pure-python-recorder/pyproject.toml", "tools/python_source_identity.py"], outputs = @[sourceManifest], declaredOutputs = @[sourceManifest],
      toolIdentityRefs = @["python-recorder-python-sdk", "python-recorder-uv-sdk"], dependencyPolicy = automaticMonitorPolicy(), cacheable = false)
    let externalCache = base / "external/cache"
    let externalManifest = base / "external/manifest.json"
    let fetchCall = publicCliCall("python-recorder-python-sdk", "python-recorder-python-sdk", "", "python.external-fetch", @[
      cliArgSeq("args", @["tools/python_external_cache_provisioning.py", "--root", root, "--cache", externalCache, "--scratch", base / "external/provisioner", "--manifest", externalManifest, "--source-manifest", sourceManifest], kind = cpkPositional)])
    let externalFetch = buildAction("python.external-fetch", fetchCall, deps = @[sourceIdentity.id],
      inputs = sdkInputs & @[sourceManifest, "tools/python_external_cache_provisioning.py", "codetracer-python-recorder/Cargo.toml", "uv.lock", "pyproject.toml", "codetracer-python-recorder/pyproject.toml", "codetracer-pure-python-recorder/pyproject.toml"],
      outputs = @[externalCache, externalManifest], declaredOutputs = @[externalCache, externalManifest],
      scratchDirs = @[base / "external/provisioner"], toolIdentityRefs = @["python-recorder-python-sdk", "python-recorder-uv-sdk"],
      dependencyPolicy = automaticMonitorPolicy(), cacheable = false)
    let runtime = base / "runtime"
    let runtimeManifest = base / "runtime-manifest.json"
    let assemblyCall = publicCliCall("python-recorder-python-sdk", "python-recorder-python-sdk", "", "python.runtime-assembly", @[
      cliArgSeq("args", @["tools/python_runtime_assembly.py", "--root", root, "--runtime", runtime, "--scratch", base / "assembly", "--manifest", runtimeManifest, "--native-wheels", base / "native-test/wheels", "--pure-wheels", base / "pure/wheels", "--external-cache", externalCache, "--external-manifest", externalManifest, "--source-manifest", sourceManifest], kind = cpkPositional)])
    let runtimeAssembly = buildAction("python.runtime-assembly", assemblyCall, deps = @[externalFetch.id, nativeWheel.id, pureWheel.id],
      inputs = sdkInputs & @["tools/python_runtime_assembly.py", sourceManifest, externalCache, externalManifest, base / "native-test/wheels", base / "pure/wheels", "uv.lock", "pyproject.toml", "codetracer-python-recorder/pyproject.toml", "codetracer-python-recorder/Cargo.toml", "codetracer-pure-python-recorder/pyproject.toml"],
      outputs = @[runtime, runtimeManifest], declaredOutputs = @[runtime, runtimeManifest], scratchDirs = @[base / "assembly"],
      toolIdentityRefs = @["python-recorder-python-sdk", "python-recorder-uv-sdk"], dependencyPolicy = automaticMonitorPolicy(), cacheable = false)
    const nimRoot = "../codetracer-trace-format-nim"
    let decoder = buildCtPrint(nimRoot)
    let decoderArtifact = ctPrintPath(nimRoot)
    let testInputs = nativeInputs & @[runtime, runtimeManifest, decoderArtifact, "codetracer-python-recorder/tests", "codetracer-pure-python-recorder", "tools/python-sdk/default.nix", ".git/index"]
    let runtimeEnv = @[("PYTHONDONTWRITEBYTECODE", "1"), ("UV_PROJECT_ENVIRONMENT", root / runtime), ("PYO3_PYTHON", root / runtime / "bin/python"), ("CARGO_BUILD_JOBS", "2")]
    let nextestCall = publicCliCall("python-recorder-uv-sdk", "python-recorder-uv-sdk", "", "python.native-tests", @[
      cliArgSeq("args", @["run", "--no-sync", "cargo", "nextest", "run", "--manifest-path", "codetracer-python-recorder/Cargo.toml", "--workspace", "--no-default-features"], kind = cpkPositional)])
    let nativeTests = buildAction("python.native-tests", nextestCall, deps = @[runtimeAssembly.id, decoder.id],
      inputs = testInputs, scratchDirs = @[base / "native-tests/cargo", base / "native-tests/uv-cache"],
      env = runtimeEnv & @[("CARGO_TARGET_DIR", root / base / "native-tests/cargo"), ("UV_CACHE_DIR", root / base / "native-tests/uv-cache")],
      toolIdentityRefs = nativeRefs & @["python-recorder-uv-sdk", "cargo-nextest"], dependencyPolicy = automaticMonitorPolicy(), cacheable = false)
    let pytestCall = publicCliCall("python-recorder-uv-sdk", "python-recorder-uv-sdk", "", "python.full-pytest", @[
      cliArgSeq("args", @["run", "--no-sync", "--group", "dev", "--group", "test", "--group", "web", "python", "-m", "pytest", "codetracer-python-recorder/tests/python", "codetracer-pure-python-recorder", "--basetemp", base / "pytest/tmp", "-o", "cache_dir=" & (base / "pytest/cache")], kind = cpkPositional)])
    let pytest = buildAction("python.full-pytest", pytestCall, deps = @[runtimeAssembly.id, decoder.id],
      inputs = testInputs, scratchDirs = @[base / "pytest", ".repro/build/python-installed-wheel-tests"], env = runtimeEnv & @[("TMPDIR", root / base / "pytest"), ("UV_CACHE_DIR", root / base / "pytest/uv-cache"), ("PYTHONDONTWRITEBYTECODE", "1"), ("CODETRACER_TRACE_FILTER_PERF", "1"), ("CODETRACER_TRACE_FILTER_PERF_OUTPUT", root / base / "pytest/trace_filter_py.json")],
      toolIdentityRefs = nativeRefs & @["python-recorder-uv-sdk", "bash"], dependencyPolicy = automaticMonitorPolicy(), cacheable = false)
    let guardCall = publicCliCall("bash", "bash", "", "python.cli-guard", @[
      cliArgSeq("args", @["codetracer-python-recorder/tests/verify-cli-convention-no-silent-skip.sh"], kind = cpkPositional)])
    let guard = buildAction("python.cli-guard", guardCall, deps = @[runtimeAssembly.id],
      inputs = testInputs & @["codetracer-python-recorder/tests/verify-cli-convention-no-silent-skip.sh"],
      env = runtimeEnv & @[("PYTHON_RECORDER_PYTHON", root / runtime / "bin/python")],
      toolIdentityRefs = @["bash", "dirname", "grep", "python-recorder-python-sdk"], dependencyPolicy = automaticMonitorPolicy(), cacheable = false)
    let rustBenchCall = publicCliCall("python-recorder-uv-sdk", "python-recorder-uv-sdk", "", "python.rust-bench", @[
      cliArgSeq("args", @["run", "--no-sync", "cargo", "bench", "--manifest-path", "codetracer-python-recorder/Cargo.toml", "--no-default-features", "--bench", "trace_filter"], kind = cpkPositional)])
    let rustBench = buildAction("python.rust-bench", rustBenchCall, deps = @[runtimeAssembly.id],
      inputs = nativeInputs & @[runtime, runtimeManifest, "codetracer-python-recorder/benches"],
      outputs = @[base / "bench/cargo"], declaredOutputs = @[base / "bench/cargo"], scratchDirs = @[base / "bench/rust-uv-cache"],
      env = runtimeEnv & @[("CARGO_TARGET_DIR", root / base / "bench/cargo"), ("UV_CACHE_DIR", root / base / "bench/rust-uv-cache")],
      toolIdentityRefs = nativeRefs & @["python-recorder-uv-sdk"], dependencyPolicy = automaticMonitorPolicy(), cacheable = false)
    let pyBenchCall = publicCliCall("python-recorder-uv-sdk", "python-recorder-uv-sdk", "", "python.python-bench", @[
      cliArgSeq("args", @["run", "--no-sync", "--group", "dev", "--group", "test", "python", "-m", "pytest", "codetracer-python-recorder/tests/python/perf/test_trace_filter_perf.py", "-q", "--basetemp", base / "bench/python/tmp", "-o", "cache_dir=" & (base / "bench/python/cache")], kind = cpkPositional)])
    let pyBench = buildAction("python.python-bench", pyBenchCall, deps = @[runtimeAssembly.id, decoder.id],
      inputs = testInputs, outputs = @[base / "bench/python/trace_filter_py.json"], declaredOutputs = @[base / "bench/python/trace_filter_py.json"],
      scratchDirs = @[base / "bench/python/tmp", base / "bench/python/uv-cache", base / "bench/python/cache"], env = runtimeEnv & @[("TMPDIR", root / base / "bench/python/tmp"), ("UV_CACHE_DIR", root / base / "bench/python/uv-cache"), ("PYTHONDONTWRITEBYTECODE", "1"), ("CODETRACER_TRACE_FILTER_PERF", "1"), ("CODETRACER_TRACE_FILTER_PERF_OUTPUT", root / base / "bench/python/trace_filter_py.json")],
      toolIdentityRefs = nativeRefs & @["python-recorder-uv-sdk"], dependencyPolicy = automaticMonitorPolicy(), cacheable = false)
    discard collect("default", @[shipping, pureWheel])
    discard collect("test", @[nativeTests, pytest, guard])
    discard collect("cargo-test", @[nativeTests])
    discard collect("bench", @[rustBench, pyBench])
    # Local source, artifact ownership and full-surface controls are qualified.
    # Windows SDK/native platform, full integration and mandatory standard-seven
    # hook qualification remain required; monitored network loss is retained
    # as an explicit noncached artifact boundary, not complete network capture.
