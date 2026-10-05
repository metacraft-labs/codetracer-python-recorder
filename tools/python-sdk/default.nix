# Own locked Python3.12 backend and native frontends, with immutable SDK identity.
{
  system ? builtins.currentSystem,
}:
let
  lockBytes = builtins.readFile ../../flake.lock;
  lock = builtins.fromJSON lockBytes;
  key = lock.nodes.${lock.root}.inputs.nixpkgs;
  locked =
    assert builtins.isString key;
    lock.nodes.${key}.locked;
  source =
    assert locked.type == "github";
    builtins.fetchTree locked;
  pkgs = import source.outPath { inherit system; };
  declared = import ../../nix/python.nix { inherit (pkgs) lib; };
  python = declared.packageFor pkgs;
  backend = python.withPackages (ps: [
    ps.setuptools
    ps.wheel
  ]);
  sourceBytes = builtins.toJSON [
    (builtins.readFile ./default.nix)
    lockBytes
    (builtins.readFile ../../nix/python.nix)
    (builtins.readFile ../../.python-version)
  ];
  sourceId = builtins.hashString "sha256" sourceBytes;
  identity = builtins.toJSON {
    inherit sourceId;
    pythonVersion = python.version;
    uvVersion = pkgs.uv.version;
    maturinVersion = pkgs.maturin.version;
    setuptoolsVersion = python.pkgs.setuptools.version;
    wheelVersion = python.pkgs.wheel.version;
    underlyingPython = "${backend}/bin/python";
    underlyingUv = "${pkgs.uv}/bin/uv";
    underlyingMaturin = "${pkgs.maturin}/bin/maturin";
  };
in
assert declared.version == "3.12";
assert builtins.elem system [
  "x86_64-linux"
  "aarch64-linux"
  "x86_64-darwin"
  "aarch64-darwin"
];
pkgs.symlinkJoin {
  name = "codetracer-python-recorder-build-sdk";
  paths = [
    backend
    pkgs.uv
    pkgs.maturin
  ];
  nativeBuildInputs = [ pkgs.makeWrapper ];
  postBuild = ''
    printf '%s' ${pkgs.lib.escapeShellArg identity} > "$out/codetracer-python-sdk-identity.json"
    rm "$out/bin/python" "$out/bin/uv" "$out/bin/maturin"
    makeWrapper ${backend}/bin/python "$out/bin/python" \
      --set RECORDER_TEST_BUILD_SDK "$out" --set RECORDER_TEST_BUILD_SDK_SOURCE_ID ${sourceId}
    makeWrapper ${pkgs.uv}/bin/uv "$out/bin/uv" \
      --set RECORDER_TEST_BUILD_SDK "$out" --set RECORDER_TEST_BUILD_SDK_SOURCE_ID ${sourceId}
    makeWrapper ${pkgs.maturin}/bin/maturin "$out/bin/maturin" \
      --set RECORDER_TEST_BUILD_SDK "$out" --set RECORDER_TEST_BUILD_SDK_SOURCE_ID ${sourceId}
  '';
}
