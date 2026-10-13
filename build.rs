use std::env;

fn main() {
    println!("cargo:rerun-if-env-changed=FINTER_API_URL");
    println!("cargo:rerun-if-env-changed=FINTER_VERSION_URL");

    // Windows app icon
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os == "windows" {
        let mut res = winres::WindowsResource::new();
        res.set_icon("assets/app-icon.ico");
        if let Err(e) = res.compile() {
            println!("cargo:warning=Failed to compile windows resource: {}", e);
        }
    } else if target_os == "linux" {
        if let Ok(out_dir) = env::var("OUT_DIR") {
            let target_dir = std::path::Path::new(&out_dir)
                .ancestors()
                .nth(3);
            if let Some(target_dir) = target_dir {
                let _ = std::fs::copy("assets/app-icon.png", target_dir.join("finter.png"));
                let _ = std::fs::copy("assets/finter.desktop", target_dir.join("finter.desktop"));
            }
        }
    }
}
