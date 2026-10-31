# Using CodeTracer Python Packages in Other Flakes

The root flake of this repository exposes two Python packages that can be used
with `python.withPackages`:

- `codetracer-python-recorder` - Rust-backed PyO3 extension (default)
- `codetracer-pure-python-recorder` - Pure Python implementation

Build them with `codetracer.lib.mkCodetracerPackagesDefault pkgs`, from your own
`pkgs`. Do not pass `codetracer.packages.${system}.*` to `withPackages`: those
are built with this flake's nixpkgs, and `withPackages` silently leaves out a
package built for a different interpreter, so the environment builds without
the recorder in it.

`codetracer.lib.python.packageFor pkgs` is the interpreter those packages are
built for, the version named in this repository's `.python-version`.

## Example 1: Basic Usage

```nix
{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-25.11";
    codetracer.url = "github:metacraft-labs/codetracer-python-recorder/agents";
  };

  outputs = { self, nixpkgs, codetracer }:
    let
      system = "x86_64-linux";
      pkgs = import nixpkgs { inherit system; };
      python = codetracer.lib.python.packageFor pkgs;
      recorders = codetracer.lib.mkCodetracerPackagesDefault pkgs;

      # Create a Python environment with codetracer packages
      myPython = python.withPackages (ps: [
        recorders.codetracer-python-recorder
        # Or use the pure Python version:
        # recorders.codetracer-pure-python-recorder
        # Or both!
      ]);
    in {
      devShells.${system}.default = pkgs.mkShell {
        packages = [ myPython ];
      };
    };
}
```

## Example 2: Using in a Python Application Package

```nix
{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-25.11";
    codetracer.url = "github:metacraft-labs/codetracer-python-recorder/agents";
  };

  outputs = { self, nixpkgs, codetracer }:
    let
      system = "x86_64-linux";
      pkgs = import nixpkgs { inherit system; };
      python = codetracer.lib.python.packageFor pkgs;
      recorders = codetracer.lib.mkCodetracerPackagesDefault pkgs;
    in {
      packages.${system}.default = python.pkgs.buildPythonApplication {
        pname = "my-app";
        version = "0.1.0";
        pyproject = true;
        src = ./.;

        build-system = [ python.pkgs.setuptools ];

        dependencies = [
          recorders.codetracer-python-recorder
          # Add other dependencies here
        ];
      };
    };
}
```

## Example 3: Custom Python with Multiple Packages

```nix
{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-25.11";
    codetracer.url = "github:metacraft-labs/codetracer-python-recorder/agents";
  };

  outputs = { self, nixpkgs, codetracer }:
    let
      system = "x86_64-linux";
      pkgs = import nixpkgs { inherit system; };
      python = codetracer.lib.python.packageFor pkgs;
      recorders = codetracer.lib.mkCodetracerPackagesDefault pkgs;

      # Create a comprehensive Python environment
      devPython = python.withPackages (ps: [
        # CodeTracer packages
        recorders.codetracer-python-recorder
        recorders.codetracer-pure-python-recorder

        # Standard packages from nixpkgs
        ps.pytest
        ps.black
        ps.mypy
        ps.requests
      ]);
    in {
      packages.${system}.python-with-codetracer = devPython;

      devShells.${system}.default = pkgs.mkShell {
        packages = [ devPython ];
      };
    };
}
```

## Testing the Installation

After entering the shell of Example 3, which has both packages (in an
environment with only one of them, check only that one):

```bash
# Enter the dev shell
nix develop

# Verify the packages are available
python -c "import codetracer_python_recorder; print('Success!')"
python -c "import codetracer_pure_python_recorder; print('Success!')"

# Check the CLI tool
codetracer-python-recorder --help
```

## Available Outputs

Of `codetracer.lib`:

- `mkCodetracerPackagesDefault pkgs` - Both packages, from your `pkgs`, for the
  declared interpreter. This is the one to use.
- `mkCodetracerPackages pkgs python` - The same for an interpreter you choose.
- `python` - The declared interpreter: `version`, `abiTag`, `packageFor pkgs`.

Of `codetracer.packages.${system}`, built with this flake's nixpkgs, for
`nix build` and `nix run` rather than for your own `withPackages`:

- `codetracer-python-recorder` - The Rust-backed recorder, for the declared interpreter
- `codetracer-pure-python-recorder` - The pure Python recorder, for the declared interpreter
- `default` - Alias for `codetracer-python-recorder`
- `codetracer-python-recorder-python312`, `codetracer-python-recorder-python313`,
  `codetracer-pure-python-recorder-python312`, `codetracer-pure-python-recorder-python313` -
  Each package for that Python version

`codetracer.overlays.default` adds both packages to your `python3`, whatever
version that is, which may not be the declared one.

These systems are supported:
- `x86_64-linux`
- `aarch64-linux`
- `x86_64-darwin`
- `aarch64-darwin`
