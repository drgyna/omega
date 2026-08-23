fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        let out = std::path::PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
        let binary = out.join("omega-vision-ocr");
        let status = std::process::Command::new("clang")
            .args([
                "-fobjc-arc",
                "src/ocr_vision.m",
                "-framework",
                "Foundation",
                "-framework",
                "AppKit",
                "-framework",
                "Vision",
                "-framework",
                "PDFKit",
                "-o",
            ])
            .arg(&binary)
            .status()
            .expect("no se pudo iniciar clang para compilar OCR local de macOS");
        if !status.success() {
            panic!("no se pudo compilar el proveedor OCR local de macOS");
        }
        println!("cargo:rustc-env=OMEGA_VISION_OCR={}", binary.display());
        println!("cargo:rerun-if-changed=src/ocr_vision.m");
    }
    tauri_build::build()
}
