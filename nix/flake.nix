{
  description = "Development environment for CodeTracer recorders (pure-python and rust-backed)";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-25.05";

  outputs = { self, nixpkgs }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" "x86_64-darwin" "aarch64-darwin" ];
      forEachSystem = nixpkgs.lib.genAttrs systems;

      # Helper function to build the recorder packages for a given Python interpreter
      mkCodetracerPackages = pkgs: python: let
        # Read versions from pyproject.toml files
        purePythonProjectToml = builtins.fromTOML (builtins.readFile ../codetracer-pure-python-recorder/pyproject.toml);
        rustBackedProjectToml = builtins.fromTOML (builtins.readFile ../codetracer-python-recorder/pyproject.toml);
      in {
        # Pure Python recorder package
        codetracer-pure-python-recorder = python.pkgs.buildPythonPackage {
          pname = "codetracer-pure-python-recorder";
          version = purePythonProjectToml.project.version;
          pyproject = true;

          src = ../codetracer-pure-python-recorder;

          build-system = with python.pkgs; [
            setuptools
          ];

          pythonImportsCheck = [ "codetracer_pure_python_recorder" ];

          meta = {
            description = "Pure-Python prototype recorder producing CodeTracer traces";
            license = pkgs.lib.licenses.mit;
          };
        };

        # Rust-backed recorder package
        codetracer-python-recorder = python.pkgs.buildPythonPackage {
          pname = "codetracer-python-recorder";
          version = rustBackedProjectToml.project.version;
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
      };

    in {
      # Expose the helper function for advanced users who want to build for custom Python versions
      lib.mkCodetracerPackages = mkCodetracerPackages;

      packages = forEachSystem (system:
        let
          pkgs = import nixpkgs { inherit system; };

          # Default packages use pkgs.python3 (follows nixpkgs default)
          defaultPackages = mkCodetracerPackages pkgs pkgs.python3;

          # Also provide version-specific packages for users who need them
          python312Packages = mkCodetracerPackages pkgs pkgs.python312;
          python313Packages = mkCodetracerPackages pkgs pkgs.python313;

        in {
          # Default packages (use nixpkgs default Python)
          inherit (defaultPackages) codetracer-pure-python-recorder codetracer-python-recorder;
          default = defaultPackages.codetracer-python-recorder;

          # Version-specific packages
          codetracer-python-recorder-python312 = python312Packages.codetracer-python-recorder;
          codetracer-python-recorder-python313 = python313Packages.codetracer-python-recorder;
          codetracer-pure-python-recorder-python312 = python312Packages.codetracer-pure-python-recorder;
          codetracer-pure-python-recorder-python313 = python313Packages.codetracer-pure-python-recorder;
        });

      # Overlay for easy integration into other flakes
      overlays.default = final: prev: let
        packages = mkCodetracerPackages final final.python3;
      in {
        python3 = prev.python3.override {
          packageOverrides = pyFinal: pyPrev: {
            codetracer-python-recorder = packages.codetracer-python-recorder;
            codetracer-pure-python-recorder = packages.codetracer-pure-python-recorder;
          };
        };
        python3Packages = final.python3.pkgs;
      };

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
              # cargo-llvm-cov is marked as broken in nixos-25.05 on some platforms
              # Uncomment when fixed upstream
              # cargo-llvm-cov
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
