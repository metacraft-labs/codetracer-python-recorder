{
  description = "Development environment for CodeTracer recorders (pure-python and rust-backed)";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-25.11";
    pre-commit-hooks.url = "github:cachix/git-hooks.nix";
  };

  outputs = { self, nixpkgs, pre-commit-hooks }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" "x86_64-darwin" "aarch64-darwin" ];
      forEachSystem = nixpkgs.lib.genAttrs systems;

      # THE Python version, read from ./.python-version — see nix/python.nix
      # for why the single source is a plain file rather than a nix attribute
      # (uv and env.ps1 read it too, and neither can read nix).
      #
      # This repo produces the ABI-locked artefact, so it OWNS the version.
      # Downstream reads `lib.python.*` below instead of passing an
      # interpreter in; a consumer that asserts a version at the producer is
      # exactly the inversion that let codetracer's dev shell build a 3.13
      # venv against a cpython-312 extension.
      python = import ./nix/python.nix { inherit (nixpkgs) lib; };

      # Helper function to build the recorder packages for a given Python interpreter.
      # Consumers can call this with their own nixpkgs and Python version to ensure
      # ABI compatibility.
      #
      # Prefer `mkCodetracerPackagesDefault pkgs` (below) unless you are
      # deliberately building for an interpreter other than the declared one:
      # it takes the version from `.python-version` so the caller cannot pick
      # one that disagrees with the extension this repo ships.
      mkCodetracerPackages = pkgs: python: let
        # Read versions from pyproject.toml files
        purePythonProjectToml = builtins.fromTOML (builtins.readFile ./codetracer-pure-python-recorder/pyproject.toml);
        rustBackedProjectToml = builtins.fromTOML (builtins.readFile ./codetracer-python-recorder/pyproject.toml);
      in {
        # Pure Python recorder package
        codetracer-pure-python-recorder = python.pkgs.buildPythonPackage {
          pname = "codetracer-pure-python-recorder";
          version = purePythonProjectToml.project.version;
          pyproject = true;

          src = ./codetracer-pure-python-recorder;

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

          src = ./codetracer-python-recorder;

          cargoDeps = pkgs.rustPlatform.importCargoLock {
            lockFile = ./codetracer-python-recorder/Cargo.lock;
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
      # The declared interpreter, for consumers. `lib` outputs are
      # system-independent, so a downstream flake reads these without
      # instantiating a package set:
      #
      #   inputs."codetracer-python-recorder".lib.python.version     "3.12"
      #   inputs."codetracer-python-recorder".lib.python.abiTag      "cpython-312"
      #   inputs."codetracer-python-recorder".lib.python.packageFor  pkgs -> drv
      #
      # codetracer/nix/python.nix consumes exactly this, which is what lets
      # that repo state no Python version of its own.
      lib.python = python;

      # Expose the helper function for advanced users who want to build for custom Python versions
      lib.mkCodetracerPackages = mkCodetracerPackages;

      # The same helper with the declared interpreter already supplied. This is
      # the one downstream should call: it cannot be given a version that
      # disagrees with the extension this repo ships.
      lib.mkCodetracerPackagesDefault = pkgs: mkCodetracerPackages pkgs (python.packageFor pkgs);

      packages = forEachSystem (system:
        let
          pkgs = import nixpkgs { inherit system; };

          # Default packages use the DECLARED interpreter (./.python-version),
          # not `pkgs.python3`.
          #
          # `pkgs.python3` follows nixpkgs' default, which is 3.13 on the
          # current pin and will be 3.14 on some later one — at which point
          # `nix build .#codetracer-python-recorder` would silently start
          # building for an interpreter OUTSIDE this package's own
          # `requires-python = ">=3.12,<3.14"`. A default that moves when
          # nixpkgs moves is not a default this repo chose.
          defaultPackages = mkCodetracerPackages pkgs (python.packageFor pkgs);

          # Also provide version-specific packages for users who need them.
          # These are the multi-version support matrix, a different claim from
          # "the version we build against": one of them must be the declared
          # one, and codetracer's scripts/test-python-version-alignment.sh
          # checks that it is.
          python312Packages = mkCodetracerPackages pkgs pkgs.python312;
          python313Packages = mkCodetracerPackages pkgs pkgs.python313;

        in {
          # Default packages (the interpreter declared in ./.python-version)
          inherit (defaultPackages) codetracer-pure-python-recorder codetracer-python-recorder;
          default = defaultPackages.codetracer-python-recorder;

          # Version-specific packages
          codetracer-python-recorder-python312 = python312Packages.codetracer-python-recorder;
          codetracer-python-recorder-python313 = python313Packages.codetracer-python-recorder;
          codetracer-pure-python-recorder-python312 = python312Packages.codetracer-pure-python-recorder;
          codetracer-pure-python-recorder-python313 = python313Packages.codetracer-pure-python-recorder;
        });

      # Overlay for easy integration into other flakes.
      #
      # This one is deliberately NOT switched to the declared interpreter, and
      # it is
      # the one output that `.python-version` does not govern: an overlay
      # exists to put the recorder into the CONSUMER's default interpreter, and
      # overriding `python312` here would leave `python3` — the attribute the
      # consumer actually writes — without the package, which is not an
      # overlay. Nor can it assert the two agree: nixpkgs' default `python3` is
      # 3.13 on current pins, so an assert would break the overlay for every
      # user today rather than catch anything.
      #
      # BOUNDARY, stated rather than papered over: a consumer of this overlay
      # gets the recorder built for ITS `python3`, which may not be the version
      # this repo publishes wheels for. That is a supported thing to want (it
      # is why the overlay takes `final.python3`) — but it means the overlay is
      # outside the single source. Consumers that need the ABI this repo ships
      # should use `lib.mkCodetracerPackagesDefault pkgs` instead, which cannot
      # pick a different one. codetracer does exactly that.
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

      checks = forEachSystem (system: {
        pre-commit-check = pre-commit-hooks.lib.${system}.run {
          src = ./.;
          hooks = {
            lint = {
              enable = true;
              name = "Lint";
              entry = "just lint";
              language = "system";
              pass_filenames = false;
            };
          };
        };
      });

      devShells = forEachSystem (system:
        let
          pkgs = import nixpkgs { inherit system; };
          preCommit = self.checks.${system}.pre-commit-check;
          declaredPython = python.packageFor pkgs;
          pureRecorderPkg =
            (mkCodetracerPackages pkgs declaredPython).codetracer-pure-python-recorder;
        in {
          # Minimal shell for running the pure-Python recorder in downstream
          # projects (e.g. CodeTracer flow tests on macOS without a full nix
          # dev shell). Provides the DECLARED interpreter (./.python-version)
          # plus the recorder pre-installed. It used to name python312 three
          # times; those were three more places the version could drift from
          # the extension this repo ships.
          python-recorder = pkgs.mkShell {
            packages = [
              (declaredPython.withPackages (_: [ pureRecorderPkg ]))
            ];
            shellHook = ''
              export CODETRACER_PYTHON_CMD="${declaredPython}/bin/python3"
              export CODETRACER_PYTHON_VERSION="${python.version}"
              export CODETRACER_PYTHON_ABI_TAG="${python.abiTag}"
              export CODETRACER_PYTHON_RECORDER_PATH="${./codetracer-pure-python-recorder/src/trace.py}"
            '';
          };

          default = pkgs.mkShell {
            packages = [
              # The declared interpreter FIRST, so a bare `python3` in this
              # shell is the one ./.python-version names. It used to be
              # python310 — the first entry of the matrix below — which meant
              # `python3 --version` in this shell answered 3.10 while
              # everything the repo builds targets 3.12.
              declaredPython
            ]
            ++ (with pkgs; [
              bashInteractive

              # The multi-version SUPPORT MATRIX. A different claim from "the
              # version we build against": these are the interpreters `just
              # test-all` / `uv sync -p` can be pointed at. `declaredPython` is
              # one of them, and codetracer's
              # scripts/test-python-version-alignment.sh checks that it is.
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
              llvmPackages_latest.llvm

              # Build tooling for Python extensions
              maturin
              uv
              pkg-config

              # Nim toolchain for the codetracer_trace_writer_nim
              # build.rs (compiles the Nim FFI sources from the
              # codetracer-trace-format-nim sibling repo to a static
              # library that the Rust crate links).
              nim
              nimble

              # CapNProto
              capnproto

              # zstd headers + libs -- nim's codetracer_trace_writer
              # imports zstd_seekable_bindings.nim which #include
              # <zstd.h> via nim's C backend; without zstd.dev on the
              # PATH the `nim c` of ct-print fails with 'zstd.h: No
              # such file or directory'.
              zstd
              zstd.dev

              # Benchmark visualisation
              gnuplot

              # Nix formatter
              nixfmt-rfc-style
            ])
            # `prek` replaces the legacy `pre-commit` workflow (workspace
            # prek-migration directive 2026-05).
            ++ [ pkgs.prek ]
            # cargo-llvm-cov is a coverage-only tool that is marked broken on
            # aarch64-darwin in the pinned nixpkgs. Including it unconditionally
            # made the whole devShell fail to evaluate under `direnv`/`nix
            # develop` on macOS (the shell silently fell back to the host
            # environment — no uv/maturin/python3.13). Gate it to non-darwin so a
            # plain `direnv exec` yields a working build/record/test shell on
            # macOS; Linux CI keeps coverage.
            ++ pkgs.lib.optionals (!pkgs.stdenv.isDarwin) [ pkgs.cargo-llvm-cov ]
            ++ preCommit.enabledPackages;

            shellHook = ''
              # When having more than one python version in the shell this variable breaks `maturin build`
              # because it always leads to having SOABI be the one from the highest version
              unset PYTHONPATH

              # Publish the declared interpreter so scripts in this shell never
              # have to resolve `python3` from PATH or restate a version. Same
              # names codetracer's dev shell exports, so a script that reads
              # them works in either.
              export CODETRACER_PYTHON_VERSION="${python.version}"
              export CODETRACER_PYTHON_ABI_TAG="${python.abiTag}"
              export CODETRACER_PYTHON_CMD="${declaredPython}/bin/python3"

              ${preCommit.shellHook}
            '';
          };
        });
    };
}
