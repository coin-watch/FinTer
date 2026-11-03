use std::env;
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-env-changed=FINTER_VERSION_URL");
    let version_url = env::var("FINTER_VERSION_URL").unwrap_or_default();
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR is set by Cargo"));
    std::fs::write(
        out_dir.join("version_url.rs"),
        format!("const DEFAULT_VERSION_URL: &str = {version_url:?};\n"),
    )
    .expect("failed to write version_url.rs");
}
