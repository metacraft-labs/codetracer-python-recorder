"""Owned locked external dependency fetch frontend; no mocks or builders.

Real immutable SDK UV fetches the complete locked external group closure.
The native action retains automatic monitoring and cacheable=false because
immutable766 documents incomplete network scope for native fetch actions.
This manifest proves source/artifact integrity, not complete network capture.
"""
from python_sdk_identity import owning_sdk
import argparse, hashlib, json, os, pathlib, shutil, subprocess
def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def cache_link_target(source, path):
    # UV emits absolute links inside its owned cache. Canonicalize only those
    # contained links so a private copy cannot refer back to the source cache.
    target = pathlib.Path(os.path.abspath(path.parent / os.readlink(path)))
    target.relative_to(source.resolve())
    resolved = path.resolve(strict=True)
    resolved.relative_to(source.resolve())
    if not (resolved.is_file() or resolved.is_dir()):
        raise RuntimeError("unsupported cache symlink target")
    return os.path.relpath(target, path.parent)

def cache_inventory(source):
    source=source.resolve()
    if not source.is_dir():raise RuntimeError("missing provisioned external dependency cache")
    records={}
    for path in sorted(source.rglob("*")):
        key=path.relative_to(source).as_posix()
        if path.is_symlink():
            target=cache_link_target(source, path)
            records[key]={"kind":"symlink","target":target,"mode":path.lstat().st_mode & 0o7777}
        elif path.is_file():
            records[key]={"kind":"file","sha256":digest(path),"size":path.stat().st_size,"mode":path.stat().st_mode & 0o7777}
        elif path.is_dir():records[key]={"kind":"directory","mode":path.stat().st_mode & 0o7777}
        else:raise RuntimeError("unsupported external cache artifact")
    if not records:raise RuntimeError("empty external dependency cache")
    return records


def provision(args):
    root=pathlib.Path(args.root).resolve();sdk,identity=owning_sdk(root)
    lock=root/"uv.lock";before=digest(lock)
    expected=json.loads(pathlib.Path(args.source_manifest).read_text())
    if before!=expected["uvLockSha256"] or identity["sourceId"]!=expected["sdkSourceId"]:
        raise RuntimeError("UV lock/SDK identity mismatch before provisioning")
    actual_metadata={name:digest(root/name) for name in ["pyproject.toml","codetracer-python-recorder/pyproject.toml","codetracer-python-recorder/Cargo.toml","codetracer-pure-python-recorder/pyproject.toml"]}
    if expected["workspaceMetadataSha256"]!=actual_metadata:raise RuntimeError("workspace metadata source identity mismatch")
    base=root/".repro/build/python-complete-graph"
    cache=pathlib.Path(args.cache).resolve();scratch=pathlib.Path(args.scratch).resolve();manifest=pathlib.Path(args.manifest).resolve()
    for path in [cache,scratch,manifest]:
        path.relative_to(base)
        if path==base:raise RuntimeError("cannot own the graph base")
    if manifest==cache or manifest==scratch or cache in manifest.parents or scratch in manifest.parents or manifest in cache.parents or manifest in scratch.parents:
        raise RuntimeError("manifest overlaps deletable cache/scratch")
    source_manifest=pathlib.Path(args.source_manifest).resolve()
    for output in [cache,scratch,manifest]:
        if output==source_manifest or output in source_manifest.parents or source_manifest in output.parents:
            raise RuntimeError("external fetch output overlaps source manifest input")
    if cache==scratch or cache in scratch.parents or scratch in cache.parents:raise RuntimeError("cache/scratch ownership overlap")
    if cache.exists():shutil.rmtree(cache)
    if scratch.exists():shutil.rmtree(scratch)
    cache.mkdir(parents=True);scratch.mkdir(parents=True)
    environment=dict(os.environ,UV_CACHE_DIR=str(cache),UV_PROJECT_ENVIRONMENT=str(scratch/"venv"),PYTHONDONTWRITEBYTECODE="1")
    command=[str(sdk/"bin/uv"),"sync","--locked","--all-groups","--no-install-workspace","--no-build","--python",str(sdk/"bin/python"),"--no-managed-python","--no-python-downloads"]
    subprocess.run(command,cwd=root,env=environment,check=True)
    if digest(lock)!=before:raise RuntimeError("locked external provisioning mutated uv.lock")
    manifest.parent.mkdir(parents=True,exist_ok=True)
    manifest.write_text(json.dumps({"uvLockSha256":before,"sdkSourceId":identity["sourceId"],"workspaceMetadataSha256":actual_metadata,"sdk":identity,"command":command,"cacheInventory":cache_inventory(cache),"evidenceBoundary":"artifact-integrity; native network monitor unknown scope retained; cacheable=false"},indent=2))

if __name__=="__main__":
    parser=argparse.ArgumentParser()
    for name in ["root","cache","scratch","manifest","source-manifest"]:parser.add_argument("--"+name,required=True)
    provision(parser.parse_args())
