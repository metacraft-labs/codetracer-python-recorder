"""Byte-exact tracked source archive capability probe; no mocks or builders.

This staging helper invokes only real Git inventory and filesystem/archive
operations. A separate native UV action performs the genuine PEP517 build.
The archive is a source transport artifact, not a fabricated published sdist.
"""
import argparse, hashlib, io, json, os, pathlib, subprocess, tarfile, tomllib

parser=argparse.ArgumentParser()
parser.add_argument("--root",required=True);parser.add_argument("--archive",required=True);parser.add_argument("--manifest",required=True)
args=parser.parse_args();root=pathlib.Path(args.root).resolve();package=root/"codetracer-pure-python-recorder"
meta=tomllib.loads((package/"pyproject.toml").read_text())["project"]
if meta["name"]!="codetracer-pure-python-recorder" or not isinstance(meta["version"],str):
 raise RuntimeError("owning pure package identity changed")
if any(c not in "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789._+-" for c in meta["version"]):
 raise RuntimeError("unsafe package version in source archive root")
tracked=subprocess.check_output(["git","-C",str(root),"ls-files","-z","--",package.name]).decode().split("\0")
archive=pathlib.Path(args.archive).resolve();manifest_path=pathlib.Path(args.manifest).resolve()
for owned in [archive,manifest_path]:
 owned.relative_to(root/".repro/build/python-complete-graph")
 if owned==root/".repro/build/python-complete-graph":raise RuntimeError("cannot own graph base")
 if owned==package or package in owned.parents or owned in package.parents:raise RuntimeError("output overlaps package input")
if archive==manifest_path or archive in manifest_path.parents or manifest_path in archive.parents:raise RuntimeError("archive and manifest overlap")
archive.parent.mkdir(parents=True,exist_ok=True);manifest_path.parent.mkdir(parents=True,exist_ok=True);manifest={};prefix="codetracer_pure_python_recorder-"+meta["version"]
with tarfile.open(archive,"w:gz") as output:
 for relative in [p for p in tracked if p]:
  source=root/relative;member=prefix+"/"+str(source.relative_to(package));info=output.gettarinfo(str(source),arcname=member);info.uid=info.gid=0;info.uname=info.gname="";info.mtime=0
  if source.is_symlink():
   target=os.readlink(source)
   if pathlib.Path(target).is_absolute():raise RuntimeError("absolute tracked source symlink")
   source.resolve(strict=True).relative_to(package.resolve());manifest[relative]={"symlink":target};output.addfile(info)
  else:
   if not source.is_file():raise RuntimeError("nonregular tracked source artifact")
   data=source.read_bytes();manifest[relative]={"sha256":hashlib.sha256(data).hexdigest()};output.addfile(info,io.BytesIO(data))
if not manifest:raise RuntimeError("empty tracked source inventory")
manifest_path.write_text(json.dumps({"sourceFiles":manifest,"archiveSha256":hashlib.sha256(archive.read_bytes()).hexdigest(),"package":meta["name"],"version":meta["version"],"kind":"tracked-source-archive-not-shipping-sdist"},indent=2))
