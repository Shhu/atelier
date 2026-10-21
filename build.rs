// On Windows, the .exe embeds ONNX Runtime, DirectML and the Visual C++ DLLs
// ONNX Runtime needs (gathered by packaging/windows/fetch-runtime.sh): they are
// extracted and loaded on the first AI selection. The .exe also gets the app
// icon (assets/icon.ico, rendered from assets/icon.svg).
fn main() {
    let target = std::env::var("TARGET").unwrap_or_default();
    if !target.contains("windows") {
        return;
    }
    let dir = std::path::Path::new("packaging/windows/runtime");
    for file in [
        "onnxruntime.dll",
        "DirectML.dll",
        "vcruntime140.dll",
        "vcruntime140_1.dll",
        "msvcp140.dll",
        "msvcp140_1.dll",
    ] {
        let path = dir.join(file);
        println!("cargo:rerun-if-changed={}", path.display());
        assert!(
            path.is_file(),
            "{} missing: run packaging/windows/fetch-runtime.sh",
            path.display()
        );
    }
    println!("cargo:rerun-if-changed=assets/icon.ico");
    winresource::WindowsResource::new()
        .set_icon("assets/icon.ico")
        .set("ProductName", "Atelier")
        .set("FileDescription", "Atelier")
        .compile()
        .expect("Windows resources (icon) compiled");
}
