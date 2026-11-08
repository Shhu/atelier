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
    // The .exe's own DLL imports are looked for in System32 only, never
    // next to it (main.rs restricts the later loads the same way).
    if target.contains("msvc") {
        println!("cargo:rustc-link-arg-bins=/DEPENDENTLOADFLAG:0x800");
    }
    println!("cargo:rerun-if-changed=assets/icon.ico");
    winresource::WindowsResource::new()
        .set_icon("assets/icon.ico")
        .set("ProductName", "Atelier")
        .set("FileDescription", "Atelier")
        .set("InternalName", "atelier")
        .set("OriginalFilename", "atelier.exe")
        .set("LegalCopyright", "Copyright (c) 2026 The Atelier contributors")
        .set_manifest(MANIFEST)
        .compile()
        .expect("Windows resources (icon) compiled");
}

/// Windows 10 and 11, per-monitor DPI (as winit sets it), long paths, and
/// the themed common controls of the system dialogs.
const MANIFEST: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <assemblyIdentity type="win32" name="io.github.Shhu.Atelier" version="1.0.0.0"/>
  <compatibility xmlns="urn:schemas-microsoft-com:compatibility.v1">
    <application>
      <supportedOS Id="{8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a}"/>
    </application>
  </compatibility>
  <application xmlns="urn:schemas-microsoft-com:asm.v3">
    <windowsSettings>
      <dpiAware xmlns="http://schemas.microsoft.com/SMI/2005/WindowsSettings">true/pm</dpiAware>
      <dpiAwareness xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">PerMonitorV2</dpiAwareness>
      <longPathAware xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">true</longPathAware>
    </windowsSettings>
  </application>
  <dependency>
    <dependentAssembly>
      <assemblyIdentity type="win32" name="Microsoft.Windows.Common-Controls" version="6.0.0.0" processorArchitecture="*" publicKeyToken="6595b64144ccf1df" language="*"/>
    </dependentAssembly>
  </dependency>
</assembly>
"#;
