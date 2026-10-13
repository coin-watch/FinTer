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
    }
}
