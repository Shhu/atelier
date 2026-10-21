#!/bin/sh
# Gathers what Atelier embeds on Windows for AI subject selection: ONNX Runtime
# (with DirectML) and DirectML from NuGet (packages checked against their
# hash), and the Visual C++ redistributable DLLs ONNX Runtime needs.
# Output goes to packaging/windows/runtime/, embedded in the .exe at build time.
set -e
cd "$(dirname "$0")"
OUT=runtime
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

fetch() { # name version sha256
    url="https://api.nuget.org/v3-flatcontainer/$1/$2/$1.$2.nupkg"
    curl -sfL -o "$TMP/$1.nupkg" "$url"
    echo "$3  $TMP/$1.nupkg" | sha256sum -c --quiet
}
fetch microsoft.ml.onnxruntime.directml 1.24.4 \
    57e9f11b73437bef7a309496135d4c1f96b1a8e9ddba60013fa27bfc1d788681
fetch microsoft.ai.directml 1.15.4 \
    4e7cb7ddce8cf837a7a75dc029209b520ca0101470fcdf275c1f49736a3615b9

mkdir -p "$OUT/licenses"
python3 - "$TMP" "$OUT" <<'PY'
import sys, zipfile, os
tmp, out = sys.argv[1], sys.argv[2]
def take(pkg, name, dest):
    with zipfile.ZipFile(os.path.join(tmp, pkg)) as z, open(os.path.join(out, dest), "wb") as f:
        f.write(z.read(name))
take("microsoft.ml.onnxruntime.directml.nupkg", "runtimes/win-x64/native/onnxruntime.dll", "onnxruntime.dll")
take("microsoft.ml.onnxruntime.directml.nupkg", "LICENSE", "licenses/onnxruntime-LICENSE.txt")
take("microsoft.ml.onnxruntime.directml.nupkg", "ThirdPartyNotices.txt", "licenses/onnxruntime-ThirdPartyNotices.txt")
take("microsoft.ai.directml.nupkg", "bin/x64-win/DirectML.dll", "DirectML.dll")
take("microsoft.ai.directml.nupkg", "LICENSE.txt", "licenses/DirectML-LICENSE.txt")
PY

# Visual C++ redistributable DLLs (the Build Tools' "Redist" folder).
CRT=$(ls -d "/mnt/c/Program Files (x86)/Microsoft Visual Studio"/*/*/VC/Redist/MSVC/*/x64/Microsoft.VC14*.CRT | tail -1)
for dll in vcruntime140.dll vcruntime140_1.dll msvcp140.dll msvcp140_1.dll; do
    cp "$CRT/$dll" "$OUT/$dll"
done
ls -la "$OUT"
