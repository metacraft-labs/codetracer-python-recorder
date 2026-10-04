import blake3
import repro_project_dsl

const sdkSourceBytes = staticRead("tools/python-sdk/default.nix") & "\0" &
  staticRead("flake.lock") & "\0" & staticRead("nix/python.nix") & "\0" &
  staticRead(".python-version")
let sdkSourceIdentity = blake3.toHex(blake3.digest(sdkSourceBytes))

package `python-recorder-python-sdk`:
  provisioning:
    nixPackage "codetracer-python-recorder-build-sdk", executablePath = "bin/python",
      expressionFile = "tools/python-sdk/default.nix",
      lockIdentity = "owning-python3.12+setuptools+wheel:" & sdkSourceIdentity
package `python-recorder-uv-sdk`:
  provisioning:
    nixPackage "codetracer-python-recorder-build-sdk", executablePath = "bin/uv",
      expressionFile = "tools/python-sdk/default.nix",
      lockIdentity = "owning-python-native-uv:" & sdkSourceIdentity
package `python-recorder-maturin-sdk`:
  provisioning:
    nixPackage "codetracer-python-recorder-build-sdk", executablePath = "bin/maturin",
      expressionFile = "tools/python-sdk/default.nix",
      lockIdentity = "owning-python-native-maturin:" & sdkSourceIdentity
