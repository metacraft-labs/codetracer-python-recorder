{
  description = "Development environment for CodeTracer recorders (pure-python and rust-backed)";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-24.05";

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
              python3
              just
              git-lfs

              # Rust toolchain for the Rust-backed Python module
              cargo
              rustc
              rustfmt
              clippy

              # Build tooling for Python extensions
              maturin
              pkg-config
            ];
          };
        });
    };
}
