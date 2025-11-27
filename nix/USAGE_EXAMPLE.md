# Using CodeTracer Python Packages in Other Flakes

This flake now exposes two Python packages that can be used with `python.withPackages`:

- `codetracer-python-recorder` - Rust-backed PyO3 extension (default)
- `codetracer-pure-python-recorder` - Pure Python implementation

## Example 1: Basic Usage

```nix
{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-25.05";
    codetracer.url = "path:/home/zahary/metacraft/codetracer-python-recorder/nix";
    # Or from git:
    # codetracer.url = "git+https://github.com/metacraft-labs/codetracer-python-recorder?dir=nix";
  };

  outputs = { self, nixpkgs, codetracer }:
    let
      system = "x86_64-linux";
      pkgs = import nixpkgs { inherit system; };

      # Create a Python environment with codetracer packages
      myPython = pkgs.python312.withPackages (ps: [
        codetracer.packages.${system}.codetracer-python-recorder
        # Or use the pure Python version:
        # codetracer.packages.${system}.codetracer-pure-python-recorder
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
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-25.05";
    codetracer.url = "path:/home/zahary/metacraft/codetracer-python-recorder/nix";
  };

  outputs = { self, nixpkgs, codetracer }:
    let
      system = "x86_64-linux";
      pkgs = import nixpkgs { inherit system; };
    in {
      packages.${system}.default = pkgs.python312.pkgs.buildPythonApplication {
        pname = "my-app";
        version = "0.1.0";
        src = ./.;

        propagatedBuildInputs = [
          codetracer.packages.${system}.codetracer-python-recorder
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
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-25.05";
    codetracer.url = "path:/home/zahary/metacraft/codetracer-python-recorder/nix";
  };

  outputs = { self, nixpkgs, codetracer }:
    let
      system = "x86_64-linux";
      pkgs = import nixpkgs { inherit system; };

      # Create a comprehensive Python environment
      devPython = pkgs.python312.withPackages (ps: [
        # CodeTracer packages
        codetracer.packages.${system}.codetracer-python-recorder
        codetracer.packages.${system}.codetracer-pure-python-recorder

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

After entering the shell or using the package:

```bash
# Enter the dev shell
nix develop

# Verify the packages are available
python -c "import codetracer_python_recorder; print('Success!')"
python -c "import codetracer_pure_python_recorder; print('Success!')"

# Check the CLI tools
codetracer-python-recorder --help
codetracer-record --help
```

## Available Packages

- `packages.${system}.codetracer-python-recorder` - The Rust-backed recorder (default)
- `packages.${system}.codetracer-pure-python-recorder` - The pure Python recorder
- `packages.${system}.default` - Alias for `codetracer-python-recorder`

All packages are built for Python 3.12 and support these systems:
- `x86_64-linux`
- `aarch64-linux`
- `x86_64-darwin`
- `aarch64-darwin`
