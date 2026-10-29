//! The native boundary. Inventory is real; fresh installation is fail-closed
//! Execution remains disabled while the native deployment path is qualified.
use appcore::storage::{Entry, Kind, Snapshot};
use std::collections::BTreeSet;
use std::path::PathBuf;

pub const PROVIDER_GAP: &str =
    "Installation is disabled in this build. No disk changes have been made.";
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Disk {
    pub entry: Entry,
    pub blocked: Option<String>,
}
#[derive(Clone, Debug)]
pub struct Inventory {
    pub disks: Vec<Disk>,
    pub snapshot: Snapshot,
}
impl Inventory {
    pub fn from_snapshot(snapshot: Snapshot) -> Self {
        let mut protected = BTreeSet::new();
        let mut system_disks = BTreeSet::new();
        let mut source_known = false;
        let mut unresolved_source = false;
        for volume in &snapshot.entries {
            if volume.mount.as_deref().is_some_and(|p| !p.is_empty()) {
                let mut resolved = false;
                let mut current = volume;
                let mut seen = BTreeSet::new();
                while seen.insert(current.id) {
                    if current.kind == Kind::Disk {
                        protected.insert(current.id);
                        if matches!(volume.mount.as_deref(), Some("/" | "/system" | "/boot")) {
                            system_disks.insert(current.id);
                        }
                        resolved = true;
                        if volume.mount.as_deref() == Some("/") {
                            source_known = true;
                        }
                        break;
                    }
                    let Some(parent) = current
                        .parent
                        .and_then(|id| snapshot.entries.iter().find(|e| e.id == id))
                    else {
                        break;
                    };
                    current = parent;
                }
                unresolved_source |= !resolved;
            }
        }
        let disks = snapshot
            .entries
            .iter()
            .filter(|e| e.kind == Kind::Disk)
            .map(|entry| {
                let blocked = if system_disks.contains(&entry.id) {
                    Some("This disk contains the running system or installation media.".into())
                } else if protected.contains(&entry.id) {
                    Some("This disk has a mounted volume and is in use.".into())
                } else if !source_known
                    || unresolved_source
                    || snapshot.physical_inventory_unavailable
                {
                    Some("The installation source cannot be identified safely.".into())
                } else if entry.read_only != Some(false) {
                    Some("Write access is read-only or unknown.".into())
                } else if entry.capacity == 0
                    || entry.sector_bytes == 0
                    || entry.capacity_from_partition_map
                {
                    Some("Hardware capacity cannot be verified.".into())
                } else {
                    None
                };
                Disk {
                    entry: entry.clone(),
                    blocked,
                }
            })
            .collect();
        Self { disks, snapshot }
    }
    pub fn target(&self, id: u64) -> Option<&Disk> {
        self.disks
            .iter()
            .find(|d| d.entry.id == id && d.blocked.is_none())
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    pub target: Entry,
    pub release: String,
    pub partitions: Vec<String>,
    pub formats: Vec<String>,
    pub erased_data: String,
    pub steps: Vec<String>,
    /// Source/target identity retained by Installer; not a raw disk ID.
    pub identity: String,
}
#[derive(Clone, Debug)]
pub struct Progress {
    pub stage: String,
    pub completed: Option<u64>,
    pub total: Option<u64>,
}
impl Progress {
    pub fn stage(stage: impl Into<String>) -> Self {
        Self {
            stage: stage.into(),
            completed: None,
            total: None,
        }
    }
}
/// Returned after payload read-back; Data is either privately opened or mounted.
#[derive(Clone, Debug)]
pub struct VerifiedTarget {
    pub id: u64,
    pub release: String,
    pub mounted_root: Option<PathBuf>,
}
pub trait Backend: Send + Sync {
    fn inventory(&self) -> Result<Inventory, String>;
    /// 署名済みソースと現在のディスク情報を検証し、変更予定を返します。
    /// ワーカー上で呼ばれます。対象ディスクへの書き込みは行いません。
    /// 検証失敗や未対応の場合はErrを返し、使用可能な計画として扱わせません。
    fn prepare(&self, disk: &Disk, release: &str) -> Result<Plan, String>;
    fn revalidate(&self, plan: &Plan) -> Result<(), String>;
    fn install(
        &self,
        plan: &Plan,
        progress: &mut dyn FnMut(Progress),
    ) -> Result<VerifiedTarget, String>;
    fn finalize(
        &self,
        target: &VerifiedTarget,
        progress: &mut dyn FnMut(Progress),
    ) -> Result<(), String>;
    fn persist_consent(
        &self,
        target: &VerifiedTarget,
        record: &mochios_device_consent::ConsentRecord,
    ) -> Result<(), String> {
        let root = target
            .mounted_root
            .as_ref()
            .filter(|p| p.is_absolute() && p.parent().is_some())
            .ok_or("The installation target is not available for privacy settings.")?;
        let store =
            mochios_device_consent::ConsentStore::at_directory(root.join("var/config/privacy"));
        store
            .save(record)
            .map_err(|e| format!("Device privacy settings could not be saved: {e}"))?;
        if store
            .load()
            .map_err(|e| format!("Device privacy settings could not be verified: {e}"))?
            != *record
        {
            return Err("Saved device privacy settings do not match the confirmed choices.".into());
        }
        Ok(())
    }
    fn supports_reboot(&self) -> bool {
        false
    }
    fn reboot(&self) -> Result<(), String>;
}
pub use crate::native::NativeBackend;
