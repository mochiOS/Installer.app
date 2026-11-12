//! Installer resource location; document loading is shared with system setup.
pub use mochios_device_consent::documents::*;
use std::path::PathBuf;
pub fn resource_root() -> PathBuf {
    #[cfg(target_os = "mochios")]
    {
        PathBuf::from("/system/applications/Installer.app/legal")
    }
    #[cfg(not(target_os = "mochios"))]
    {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("legal")
    }
}
