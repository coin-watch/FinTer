#![allow(dead_code)]
pub mod data {
    pub mod client {
        pub fn tokio_runtime() -> &'static tokio::runtime::Runtime {
            static RT: std::sync::OnceLock<tokio::runtime::Runtime> = std::sync::OnceLock::new();
            RT.get_or_init(|| tokio::runtime::Runtime::new().unwrap())
        }
    }
    pub mod error { include!("../../../src/data/error.rs"); }
    pub mod version { include!("../../../src/data/version.rs"); }
    pub mod update_checker { include!("../../../src/data/update_checker.rs"); }
    pub mod update_downloader { include!("../../../src/data/update_downloader.rs"); }
    pub mod update_manager { include!("../../../src/data/update_manager.rs"); }
}
#[test]
fn raw_artifact_extension() {
    let source = include_str!("../../../src/data/update_downloader.rs");
    assert!(!source.contains("\".AppImage\""));
    assert!(!source.contains("\".dmg\""));
}
