"""Owned native wheel installation frontend: real UV, no mocks or builders.

Compilation belongs to separate typed maturin and UV/setuptools actions.
This frontend validates their actual artifacts and installs them offline into
one owned runtime. It never invokes workspace/source package builders.
"""
import argparse, configparser, email, hashlib, importlib.metadata, json, os
import pathlib, shutil, subprocess, sys, tomllib, zipfile
from setuptools._vendor.packaging.tags import parse_tag, sys_tags
from setuptools._vendor.packaging.utils import canonicalize_name, parse_wheel_filename

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

def owned(root,path):
    path=path.resolve();base=root/".repro/build/python-complete-graph";path.relative_to(base)
    if path==base:raise RuntimeError("cannot own the entire graph base")
    return path

def overlaps(a,b):
    return a==b or a in b.parents or b in a.parents

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

def safe_copy_cache(source,destination):
    expected = cache_inventory(source)
    shutil.copytree(source,destination,symlinks=True)
    for key, record in expected.items():
        if record["kind"] == "symlink":
            path = destination / key
            path.unlink()
            path.symlink_to(record["target"])
    if cache_inventory(destination) != expected:
        raise RuntimeError("private cache copy differs from the provisioned artifacts")


def wheel_record(directory,project,backend,require_pure):
    paths=list(directory.glob("*.whl"))
    if len(paths)!=1:raise RuntimeError(f"expected one actual {project['name']} wheel, got {paths}")
    path=paths[0];name,version,build,tags=parse_wheel_filename(path.name)
    if name!=canonicalize_name(project["name"]) or str(version)!=project["version"]:
        raise RuntimeError("wheel filename identity differs from the owning package")
    with zipfile.ZipFile(path) as archive:
        metadata_names=[n for n in archive.namelist() if n.endswith(".dist-info/METADATA")]
        wheel_names=[n for n in archive.namelist() if n.endswith(".dist-info/WHEEL")]
        if len(metadata_names)!=1 or len(wheel_names)!=1:raise RuntimeError("ambiguous actual wheel metadata")
        metadata=email.message_from_bytes(archive.read(metadata_names[0]))
        shape=email.message_from_bytes(archive.read(wheel_names[0]))
        if metadata.get_all("Name")!=[project["name"]] or metadata.get_all("Version")!=[project["version"]]:
            raise RuntimeError("wheel METADATA identity differs from the owning package")
        if shape.get("Root-Is-Purelib")!=("true" if require_pure else "false"):
            raise RuntimeError("wheel native/pure role mismatch")
        declared=set()
        for tag in shape.get_all("Tag",[]):declared.update(parse_tag(tag))
        if declared!=tags or not declared.intersection(set(sys_tags())):
            raise RuntimeError("wheel ABI/platform tag is unsupported by the actual owning interpreter")
        if not shape.get("Generator","").startswith(backend+" ("):
            raise RuntimeError("wheel was not produced by its canonical backend")
        if require_pure:
            entries=[n for n in archive.namelist() if n.endswith(".dist-info/entry_points.txt")]
            if len(entries)!=1:raise RuntimeError("pure wheel is missing canonical entry points")
            config=configparser.ConfigParser();config.read_string(archive.read(entries[0]).decode())
            if dict(config["console_scripts"])!=project["scripts"]:
                raise RuntimeError("pure wheel console scripts differ from pyproject")
    return {"path":str(path),"sha256":digest(path),"name":project["name"],"version":project["version"],"tags":sorted(map(str,tags)),"generator":shape.get("Generator")}

def run(uv,args,root,environment):
    command=[str(uv),*args]
    subprocess.run(command,cwd=root,env=environment,check=True)
    return command

def assemble(args):
    root=pathlib.Path(args.root).resolve();sdk,identity=sdk_record(root)
    runtime=owned(root,pathlib.Path(args.runtime));scratch=owned(root,pathlib.Path(args.scratch))
    manifest=owned(root,pathlib.Path(args.manifest))
    if overlaps(runtime,scratch) or overlaps(manifest,runtime) or overlaps(manifest,scratch):
        raise RuntimeError("runtime/scratch/manifest ownership overlap")
    for source in [args.native_wheels,args.pure_wheels,args.external_cache,args.external_manifest,args.source_manifest]:
        source=pathlib.Path(source).resolve()
        if overlaps(source,runtime) or overlaps(source,scratch) or overlaps(source,manifest):
            raise RuntimeError("input artifact overlaps mutable assembly output")
    lock_before=digest(root/"uv.lock")
    native_project=tomllib.loads((root/"codetracer-python-recorder/pyproject.toml").read_text())["project"]
    if native_project.get("dynamic") and "version" in native_project["dynamic"]:
        native_project["version"]=tomllib.loads((root/"codetracer-python-recorder/Cargo.toml").read_text())["package"]["version"]
    pure_project=tomllib.loads((root/"codetracer-pure-python-recorder/pyproject.toml").read_text())["project"]
    native=wheel_record(pathlib.Path(args.native_wheels),native_project,"maturin",False)
    pure=wheel_record(pathlib.Path(args.pure_wheels),pure_project,"setuptools",True)
    provisioned=pathlib.Path(args.external_cache).resolve()
    cache_manifest=json.loads(pathlib.Path(args.external_manifest).read_text())
    source_manifest=json.loads(pathlib.Path(args.source_manifest).read_text())
    for field in ["uvLockSha256","sdkSourceId","workspaceMetadataSha256"]:
        if source_manifest[field]!=cache_manifest[field]:raise RuntimeError("source identity differs from fetched cache provenance")
    if cache_manifest["uvLockSha256"]!=digest(root/"uv.lock") or cache_manifest["sdkSourceId"]!=identity["sourceId"]:
        raise RuntimeError("external cache provenance differs from the owning source tuple")
    actual_metadata={name:digest(root/name) for name in ["pyproject.toml","codetracer-python-recorder/pyproject.toml","codetracer-python-recorder/Cargo.toml","codetracer-pure-python-recorder/pyproject.toml"]}
    if cache_manifest["workspaceMetadataSha256"]!=actual_metadata:raise RuntimeError("workspace metadata differs from provisioned closure")
    if cache_manifest["cacheInventory"]!=cache_inventory(provisioned):
        raise RuntimeError("external cache bytes differ from the provisioned artifact manifest")
    if runtime.exists():shutil.rmtree(runtime)
    if scratch.exists():shutil.rmtree(scratch)
    scratch.mkdir(parents=True);cache=scratch/"cache";safe_copy_cache(provisioned,cache)
    if cache_inventory(cache)!=cache_manifest["cacheInventory"]:
        raise RuntimeError("copied cache differs before UV mutation")
    environment=dict(os.environ,UV_CACHE_DIR=str(cache),UV_PROJECT_ENVIRONMENT=str(runtime),PYTHONDONTWRITEBYTECODE="1")
    uv=sdk/"bin/uv";commands=[]
    commands.append(run(uv,["sync","--locked","--all-groups","--no-install-workspace","--no-build","--python",str(sdk/"bin/python"),"--no-managed-python","--no-python-downloads","--offline"],root,environment))
    python=runtime/("Scripts/python.exe" if os.name=="nt" else "bin/python")
    for item in [native,pure]:
        commands.append(run(uv,["pip","install","--python",str(python),"--no-deps","--no-build","--offline","--no-index",item["path"]],root,environment))
    code="import importlib.metadata,json,codetracer_python_recorder,codetracer_pure_python_recorder;print(json.dumps({'nativeVersion':importlib.metadata.version('codetracer-python-recorder'),'pureVersion':importlib.metadata.version('codetracer-pure-python-recorder'),'nativeModule':codetracer_python_recorder.__file__,'pureModule':codetracer_pure_python_recorder.__file__}))"
    actual=json.loads(subprocess.check_output([str(python),"-c",code],cwd=scratch,env=environment,text=True))
    if actual["nativeVersion"]!=native["version"] or actual["pureVersion"]!=pure["version"]:
        raise RuntimeError("installed distribution versions differ from validated artifacts")
    for name in ["nativeModule","pureModule"]:pathlib.Path(actual[name]).resolve().relative_to(runtime)
    if digest(root/"uv.lock")!=lock_before:raise RuntimeError("runtime assembly mutated locked source")
    manifest.parent.mkdir(parents=True,exist_ok=True)
    manifest.write_text(json.dumps({"sdk":identity,"uvLockSha256":digest(root/"uv.lock"),"nativeWheel":native,"pureWheel":pure,"commands":commands,"installed":actual},indent=2))

parser=argparse.ArgumentParser()
for name in ["root","runtime","scratch","manifest","native-wheels","pure-wheels","external-cache","external-manifest","source-manifest"]:parser.add_argument("--"+name,required=True)
if __name__=="__main__":assemble(parser.parse_args())
