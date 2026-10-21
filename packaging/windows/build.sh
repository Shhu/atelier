#!/bin/sh
# Builds Atelier for Windows from WSL: a single .exe that embeds ONNX Runtime
# and DirectML (see fetch-runtime.sh and build.rs). Uses the Windows Rust
# toolchain of the Windows user, and builds into their profile folder
# (.atelier-target), outside WSL's file system.
set -e
cd "$(dirname "$0")/../.."
[ -f packaging/windows/runtime/onnxruntime.dll ] || packaging/windows/fetch-runtime.sh
. packaging/windows/paths.sh
"$WIN_HOME_WSL/.cargo/bin/cargo.exe" build --release --target-dir "$WIN_TARGET"
# Old DLL left next to the .exe: no longer needed.
rm -f "$WIN_TARGET_WSL/release/DirectML.dll"
printf 'Ready: %s\n' "$WIN_TARGET\\release\\atelier.exe"
