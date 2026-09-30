# The Python interpreter this repository is built and shipped against, read
# from `../.python-version` — the one place the version is written down.
#
# WHY `.python-version` AND NOT A NIX ATTRIBUTE
#
# This repo produces a PyO3 extension module, which is ABI-locked to a CPython
# minor version (`codetracer_python_recorder.cpython-312-*.so` loads into 3.12
# and nothing else). It therefore owns the version — every consumer has to
# agree with it, not the other way round. But its consumers are not all Nix:
#
#   uv        reads `.python-version` NATIVELY. `uv sync`, `uv run` and
#             `uv venv` all honour it, so `just dev` now produces a `.venv` at
#             the declared version BY DECLARATION. Before this file it produced
#             3.12 by accident — uv picked the first interpreter on PATH that
#             satisfied `requires-python`, and which one that was depended on
#             the order of the package list in this flake's devShell.
#   env.ps1   (native Windows, no Nix) resolves the base interpreter from
#             `.venv/pyvenv.cfg`'s `home =` line, i.e. from whatever uv chose.
#             It therefore follows this file too, without needing to know it
#             exists.
#   Nix       reads it here.
#   codetracer  reads it through `inputs."codetracer-python-recorder".lib`
#             (see codetracer/nix/python.nix), so the consumer no longer
#             asserts a version at the producer.
#
# A nix attribute would have covered only the last two. A plain text file that
# uv already understands covers all four, which is the difference between "the
# version is specified in one place" being true of the Nix lane and being true
# of the product.
#
# Everything below is DERIVED from that one string. Nothing restates it.
{ lib }:
let
  # The single source. `.python-version` may carry a trailing newline (every
  # tool that writes it does), so take the first line rather than trusting the
  # file to be exactly N bytes.
  version = lib.head (lib.splitString "\n" (builtins.readFile ../.python-version));

  # "312" — the nixpkgs attribute suffix and the body of the CPython ABI tag.
  nodot = builtins.replaceStrings [ "." ] [ "" ] version;
in
{
  inherit version nodot;

  # "python312" — the nixpkgs attribute name.
  attrName = "python${nodot}";

  # "cpython-312" — the PEP 425 tag a compiled extension module built for this
  # interpreter carries, as in
  # `codetracer_python_recorder.cpython-312-x86_64-linux-gnu.so`. Consumers
  # compare built artefacts against this instead of a hardcoded literal.
  abiTag = "cpython-${nodot}";

  # The interpreter itself, resolved out of a caller-supplied package set so
  # that a consumer following a different nixpkgs still gets the same MINOR
  # version. Deliberately not `pkgs.python3`: that follows nixpkgs' default,
  # which is 3.13 on the pin in use today and will be 3.14 on some later one —
  # outside this package's own `requires-python = ">=3.12,<3.14"` window.
  packageFor = pkgs: pkgs."python${nodot}";
}
