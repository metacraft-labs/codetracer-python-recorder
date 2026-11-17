"""Native source identity artifact frontend; no mocks or builders.

Records actual committed-lock source bytes and validated owning SDK identity.
Downstream fetch validates this artifact before any network/provisioning work.
"""
from python_sdk_identity import owning_sdk
import argparse, hashlib, json, pathlib
def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()



if __name__=="__main__":
    parser=argparse.ArgumentParser();parser.add_argument("--root",required=True);parser.add_argument("--manifest",required=True)
    args=parser.parse_args();root=pathlib.Path(args.root).resolve();sdk,identity=owning_sdk(root)
    manifest=pathlib.Path(args.manifest).resolve();base=root/".repro/build/python-complete-graph"
    manifest.relative_to(base)
    if manifest==base:raise RuntimeError("cannot own graph base")
    manifest.parent.mkdir(parents=True,exist_ok=True)
    manifest.write_text(json.dumps({"uvLockSha256":digest(root/"uv.lock"),"sdkSourceId":identity["sourceId"],"workspaceMetadataSha256":{name:digest(root/name) for name in ["pyproject.toml","codetracer-python-recorder/pyproject.toml","codetracer-python-recorder/Cargo.toml","codetracer-pure-python-recorder/pyproject.toml"]}},indent=2))
