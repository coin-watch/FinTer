use std::env;

fn main() {
    println!("cargo:rerun-if-env-changed=FINTER_API_URL");
    println!("cargo:rerun-if-env-changed=FINTER_VERSION_URL");

    // Bake the update endpoint into the binary. Writing a generated source file
    // (instead of relying on option_env!) makes Cargo rebuild the lib and bin
    // when the URL changes, even when a cached target directory is restored.
    let version_url = env::var("FINTER_VERSION_URL").unwrap_or_default();
    let out_dir = std::path::PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR is set by Cargo"));
    std::fs::write(
        out_dir.join("version_url.rs"),
        format!("const DEFAULT_VERSION_URL: &str = {version_url:?};\n"),
    )
    .expect("failed to write version_url.rs");

    // Windows app icon
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os == "windows" {
        let mut res = winres::WindowsResource::new();
        res.set_icon("assets/app-icon.ico");
        if let Err(e) = res.compile() {
            println!("cargo:warning=Failed to compile windows resource: {}", e);
        }
    }

    if target_os == "linux" {
        if let Ok(out_dir) = env::var("OUT_DIR") {
            if let Some(target_dir) = std::path::Path::new(&out_dir).ancestors().nth(3) {
                let _ = std::fs::copy("assets/app-icon.png", target_dir.join("finter.png"));
                let _ = std::fs::copy("assets/finter.desktop", target_dir.join("finter.desktop"));
            }
        }
    }
}
