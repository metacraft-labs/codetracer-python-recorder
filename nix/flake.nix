{
  description = "Development environment for CodeTracer recorders (pure-python and rust-backed)";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-25.05";

  outputs = { self, nixpkgs }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" "x86_64-darwin" "aarch64-darwin" ];
      forEachSystem = nixpkgs.lib.genAttrs systems;
    in {
      packages = forEachSystem (system:
        let
          pkgs = import nixpkgs { inherit system; };

          # Pure Python recorder package
          codetracer-pure-python-recorder = pkgs.python312.pkgs.buildPythonPackage {
            pname = "codetracer-pure-python-recorder";
            version = "0.1.0";
            pyproject = true;

            src = ../codetracer-pure-python-recorder;

            build-system = with pkgs.python312.pkgs; [
              setuptools
            ];

            pythonImportsCheck = [ "codetracer_pure_python_recorder" ];

            meta = {
              description = "Pure-Python prototype recorder producing CodeTracer traces";
              license = pkgs.lib.licenses.mit;
            };
          };

          # Rust-backed recorder package
          codetracer-python-recorder = pkgs.python312.pkgs.buildPythonPackage {
            pname = "codetracer-python-recorder";
            version = "0.3.0";
            pyproject = true;

            src = ../codetracer-python-recorder;

            cargoDeps = pkgs.rustPlatform.importCargoLock {
              lockFile = ../codetracer-python-recorder/Cargo.lock;
            };

            nativeBuildInputs = with pkgs; [
              rustPlatform.cargoSetupHook
              rustPlatform.maturinBuildHook
              capnproto
              pkg-config
            ];

            pythonImportsCheck = [ "codetracer_python_recorder" ];

            meta = {
              description = "Low-level Rust-backed Python module for CodeTracer recording (PyO3)";
              license = pkgs.lib.licenses.mit;
            };
          };

        in {
          inherit codetracer-pure-python-recorder codetracer-python-recorder;
          default = codetracer-python-recorder;
        });

      devShells = forEachSystem (system:
        let pkgs = import nixpkgs { inherit system; };
        in {
          default = pkgs.mkShell {
            packages = with pkgs; [
              bashInteractive
              python310
	      python311
	      python312
	      python313
              just
              git-lfs

              # Linters and type checkers for Python code
              ruff
              black
              mypy

              # Rust toolchain for the Rust-backed Python module
              cargo
              rustc
              rustfmt
              clippy
	      rust-analyzer
	      cargo-nextest
              cargo-llvm-cov
              llvmPackages_latest.llvm

              # Build tooling for Python extensions
              maturin
              uv
              pkg-config

              # CapNProto
	      capnproto

              # Benchmark visualisation
              gnuplot
            ];

	    shellHook = ''
	      # When having more than one python version in the shell this variable breaks `maturin build`
	      # because it always leads to having SOABI be the one from the highest version
	      unset PYTHONPATH
	    '';
          };
        });
    };
}
