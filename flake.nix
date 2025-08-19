{
  description = "Development environment for CodeTracer recorders (pure-python and rust-backed)";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-25.05";

  outputs = { self, nixpkgs }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" "x86_64-darwin" "aarch64-darwin" ];
      forEachSystem = nixpkgs.lib.genAttrs systems;
    in {
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

              # Build tooling for Python extensions
              maturin
              uv
              pkg-config
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
