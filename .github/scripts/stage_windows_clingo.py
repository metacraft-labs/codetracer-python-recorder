"""Stage the clingo 5.8.0 Windows runtime for reprobuild CI.

Kept as a standalone file rather than an inline PowerShell here-string in
`.github/workflows/ci-reprobuild.yml`: a here-string terminator -- and, for
Python, the whole unindented body -- must start at column 0, which terminates
the enclosing YAML block scalar and makes the workflow file unparseable.

Reads REPROBUILD_CLINGO_ROOT from the environment; that directory must already
exist.  Requires the `zstandard` package.
"""

import hashlib
import io
import os
import pathlib
import tarfile
import urllib.request
import zipfile

import zstandard

URL = "https://conda.anaconda.org/conda-forge/win-64/clingo-5.8.0-py312h4128c23_0.conda"
SHA256 = "9ffe4802f8d39991bef75a1548f10147a59e5549eca65fef5119eb281e4f343a"
ROOT = pathlib.Path(os.environ["REPROBUILD_CLINGO_ROOT"])
PKG = ROOT / "clingo.conda"

urllib.request.urlretrieve(URL, PKG)
actual = hashlib.sha256(PKG.read_bytes()).hexdigest()
if actual != SHA256:
    raise SystemExit(f"clingo package sha256 mismatch: {actual} != {SHA256}")

with zipfile.ZipFile(PKG) as archive:
    pkg_member = next(
        name for name in archive.namelist()
        if name.startswith("pkg-") and name.endswith(".tar.zst")
    )
    payload = zstandard.ZstdDecompressor().decompress(archive.read(pkg_member))

with tarfile.open(fileobj=io.BytesIO(payload), mode="r:") as tar:
    for member in tar.getmembers():
        name = member.name.replace("\\", "/")
        if not name.startswith("Library/bin/") or member.isdir():
            continue
        rel = pathlib.PurePosixPath(name).relative_to("Library")
        dest = ROOT / pathlib.Path(*rel.parts)
        dest.parent.mkdir(parents=True, exist_ok=True)
        with tar.extractfile(member) as src, dest.open("wb") as out:
            out.write(src.read())

PKG.unlink(missing_ok=True)
print(f"clingo runtime staged at {ROOT / 'bin'}")
