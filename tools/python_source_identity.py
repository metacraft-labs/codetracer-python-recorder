"""Native source identity artifact frontend; no mocks or builders.

Records actual committed-lock source bytes and validated owning SDK identity.
Downstream fetch validates this artifact before any network/provisioning work.
"""
import argparse, hashlib, importlib.metadata, json, os, pathlib, subprocess, sys
def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()

def sdk_record(root):
    sdk=pathlib.Path(os.environ["RECORDER_TEST_BUILD_SDK"]).resolve()
    sdk.relative_to("/nix/store")
    record=json.loads((sdk/"codetracer-python-sdk-identity.json").read_text())
    source_files=["tools/python-sdk/default.nix","flake.lock","nix/python.nix",".python-version"]
    encoded=json.dumps([(root/p).read_text() for p in source_files],ensure_ascii=False,separators=(",",":")).encode()
    expected=hashlib.sha256(encoded).hexdigest()
    if record["sourceId"]!=expected or os.environ["RECORDER_TEST_BUILD_SDK_SOURCE_ID"]!=expected:
        raise RuntimeError("SDK source identity mismatch")
    for field in ["underlyingPython","underlyingUv","underlyingMaturin"]:
        pathlib.Path(record[field]).resolve().relative_to("/nix/store")
    if pathlib.Path(sys.executable).resolve()!=pathlib.Path(record["underlyingPython"]).resolve():
        raise RuntimeError("frontend is not executing the owning SDK interpreter")
    if sys.version_info[:2]!=(3,12):
        raise RuntimeError("frontend ABI differs from the approved Python3.12 SDK")
    record["setuptoolsActual"]=importlib.metadata.version("setuptools")
    record["wheelActual"]=importlib.metadata.version("wheel")
    import setuptools, wheel
    record["setuptoolsModule"]=setuptools.__file__;record["wheelModule"]=wheel.__file__
    backend_root = pathlib.Path(record["underlyingPython"]).parent.parent
    for field, package in [("setuptoolsModule", "setuptools"), ("wheelModule", "wheel")]:
        actual_module = pathlib.Path(record[field]).resolve(strict=True)
        selected_module = backend_root / "lib" / "python3.12" / "site-packages" / package / "__init__.py"
        if actual_module != selected_module.resolve(strict=True):
            raise RuntimeError("backend module differs from the selected SDK package: " + package)
        actual_module.relative_to("/nix/store")
    actual=subprocess.check_output([str(sdk/"bin/uv"),"--version"],text=True).split()
    if actual[:2]!=["uv",record["uvVersion"]]:
        raise RuntimeError("UV frontend differs from the immutable SDK marker")
    return sdk,record


if __name__=="__main__":
    parser=argparse.ArgumentParser();parser.add_argument("--root",required=True);parser.add_argument("--manifest",required=True)
    args=parser.parse_args();root=pathlib.Path(args.root).resolve();sdk,identity=sdk_record(root)
    manifest=pathlib.Path(args.manifest).resolve();base=root/".repro/build/python-complete-graph"
    manifest.relative_to(base)
    if manifest==base:raise RuntimeError("cannot own graph base")
    manifest.parent.mkdir(parents=True,exist_ok=True)
    manifest.write_text(json.dumps({"uvLockSha256":digest(root/"uv.lock"),"sdkSourceId":identity["sourceId"],"workspaceMetadataSha256":{name:digest(root/name) for name in ["pyproject.toml","codetracer-python-recorder/pyproject.toml","codetracer-python-recorder/Cargo.toml","codetracer-pure-python-recorder/pyproject.toml"]}},indent=2))
