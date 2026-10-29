//! Dormant native adapter. Enabling requires a reviewed source/launch policy.
use crate::{
    backend::*,
    deployment::{Deployment, Source},
};
#[cfg(target_os = "mochios")]
use appcore::storage::StorageRegion;
use appcore::storage::{self, Observation};
#[cfg(target_os = "mochios")]
use std::sync::Arc;
use std::{path::Path, sync::Mutex};

// Intentionally not a Cargo feature, environment variable, preference or CLI flag.
// Both preparation and execution refuse before acquiring any write authority.
pub const INSTALLATION_ENABLED: bool = false;
struct Prepared {
    plan: Plan,
    source: Source,
    observation: Observation,
    epoch: u64,
}
#[derive(Default)]
struct State {
    prepared: Option<Prepared>,
    deployment: Option<(u64, String, Deployment)>,
}
#[derive(Default)]
pub struct NativeBackend {
    state: Mutex<State>,
}
fn enabled() -> Result<(), String> {
    if INSTALLATION_ENABLED {
        Ok(())
    } else {
        Err(PROVIDER_GAP.into())
    }
}
fn message(e: impl std::fmt::Debug) -> String {
    format!("Installation failed: {e:?}")
}
impl Backend for NativeBackend {
    fn inventory(&self) -> Result<Inventory, String> {
        storage::inventory()
            .map(Inventory::from_snapshot)
            .map_err(|e| match e {
                appcore::Error::UnsupportedPlatform => {
                    "Disk discovery is not available in this preview.".into()
                }
                appcore::Error::System(13) => {
                    "Installer does not have permission to read disk information.".into()
                }
                appcore::Error::System(11) => {
                    "Disk information is not ready yet. Try Refresh Disks again.".into()
                }
                _ => format!("Disk information could not be read: {e:?}"),
            })
    }
    fn prepare(&self, disk: &Disk, release: &str) -> Result<Plan, String> {
        enabled()?;
        let build = option_env!("MOCHIOS_BUILD_NUMBER")
            .ok_or("The Installer build identity is unavailable.")?
            .parse()
            .map_err(message)?;
        let source = Source::open(
            Path::new("/system/installation"),
            release,
            build,
            &[mochios_system_image::RELEASE_PUBLIC_KEY],
        )
        .map_err(message)?;
        let observation = storage::inventory_observation().map_err(message)?;
        if Inventory::from_snapshot(observation.snapshot.clone()).target(disk.entry.id)
            != Some(disk)
            || disk.entry.sector_bytes != 512
            || disk.entry.capacity < source.manifest.bytes
        {
            return Err("The selected disk changed, is in use, is too small, or has an unsupported sector size.".into());
        }
        let epoch = storage::hardware_epoch(&observation, disk.entry.id).map_err(message)?;
        let layout = source.gpt.layout;
        let parts = [
            ("EFI System", layout.esp),
            ("Boot A", layout.boot[0]),
            ("System A", layout.system[0]),
            ("Boot B", layout.boot[1]),
            ("System B", layout.system[1]),
            ("Data", layout.data),
            ("Boot State", layout.state),
        ];
        let digest = source
            .manifest
            .sha256
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>();
        let plan=Plan{target:disk.entry.clone(),release:release.into(),partitions:parts.iter().map(|(name,p)|format!("{name}: {} bytes (sectors {}–{})",p.sectors()*512,p.first_lba,p.last_lba)).collect(),
            formats:vec!["EFI System: FAT32; System A/B: ext2; Data: ext4 (from the signed template).".into()],
            erased_data:"All existing partitions and files on the selected disk will be removed. This is not a secure erase.".into(),
            steps:vec!["Verify the signed installation image and both boot slots.".into(),"Reserve the exact destination device and erase its partition metadata.".into(),"Copy and read back the bootloader, A/B slots, Data and initial Boot State.".into(),"Save and verify device privacy settings in the target Data filesystem.".into(),"Publish and verify both GPT copies. Remaining capacity stays unallocated.".into()],identity:format!("{digest}:{}:{}:{epoch}",disk.entry.id,disk.entry.capacity)};
        let mut state = self.state.lock().map_err(message)?;
        if state.deployment.is_some() {
            return Err("An installation already owns the destination.".into());
        }
        state.prepared = Some(Prepared {
            plan: plan.clone(),
            source,
            observation,
            epoch,
        });
        Ok(plan)
    }
    fn revalidate(&self, plan: &Plan) -> Result<(), String> {
        enabled()?;
        let observation = storage::inventory_observation().map_err(message)?;
        let inventory = Inventory::from_snapshot(observation.snapshot.clone());
        if !inventory
            .target(plan.target.id)
            .is_some_and(|d| d.entry == plan.target)
        {
            return Err("The installation destination changed or is in use.".into());
        }
        let mut state = self.state.lock().map_err(message)?;
        let prepared = state
            .prepared
            .as_mut()
            .filter(|p| p.plan == *plan)
            .ok_or("The reviewed installation plan is no longer available.")?;
        if storage::hardware_epoch(&observation, plan.target.id).map_err(message)? != prepared.epoch
        {
            return Err("The destination device was replaced after review.".into());
        }
        prepared.observation = observation;
        Ok(())
    }
    fn install(
        &self,
        plan: &Plan,
        progress: &mut dyn FnMut(Progress),
    ) -> Result<VerifiedTarget, String> {
        enabled()?;
        #[cfg(not(target_os = "mochios"))]
        {
            let _ = (plan, progress);
            Err("Installation is not supported in Linux preview.".into())
        }
        #[cfg(target_os = "mochios")]
        {
            let mut state = self.state.lock().map_err(message)?;
            let p = state
                .prepared
                .take()
                .filter(|p| p.plan == *plan)
                .ok_or("The reviewed installation plan is no longer available.")?;
            let endpoint = mochi_user_platform::handle::inherited(
                mochios_filesystem_protocol::CONTROL_HANDLE_KEY,
            )
            .map_err(message)?;
            let region = StorageRegion::acquire(
                endpoint,
                &p.observation,
                plan.target.id,
                0,
                plan.target.capacity / 512,
                p.epoch,
            )
            .map_err(message)?;
            let mut deployment = Deployment::new(p.source, Arc::new(region), |bytes| {
                mochi_user_platform::random::fill(bytes).map_err(|e| {
                    std::io::Error::other(format!("Random identity generation failed: {e:?}"))
                })
            })
            .map_err(message)?;
            deployment.copy(progress).map_err(message)?;
            state.deployment = Some((plan.target.id, plan.release.clone(), deployment));
            Ok(VerifiedTarget {
                id: plan.target.id,
                release: plan.release.clone(),
                mounted_root: None,
            })
        }
    }
    fn persist_consent(
        &self,
        target: &VerifiedTarget,
        record: &mochios_device_consent::ConsentRecord,
    ) -> Result<(), String> {
        enabled()?;
        let mut state = self.state.lock().map_err(message)?;
        let (_, _, deployment) = state
            .deployment
            .as_mut()
            .filter(|(id, release, _)| *id == target.id && *release == target.release)
            .ok_or("The installed target is no longer available.")?;
        deployment.save_consent(record).map_err(message)
    }
    fn finalize(
        &self,
        target: &VerifiedTarget,
        progress: &mut dyn FnMut(Progress),
    ) -> Result<(), String> {
        enabled()?;
        let mut state = self.state.lock().map_err(message)?;
        let (_, _, deployment) = state
            .deployment
            .as_mut()
            .filter(|(id, release, _)| *id == target.id && *release == target.release)
            .ok_or("The installed target is no longer available.")?;
        deployment.finish(progress).map_err(message)?;
        state.deployment = None;
        Ok(())
    }
    fn reboot(&self) -> Result<(), String> {
        Err("A supported restart API is not available to Installer.".into())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn activation_is_closed_before_io_or_authority_acquisition() {
        assert!(!INSTALLATION_ENABLED);
        let backend = NativeBackend::default();
        assert!(enabled().is_err());
        let target = VerifiedTarget {
            id: 1,
            release: "test".into(),
            mounted_root: None,
        };
        assert!(
            backend
                .persist_consent(&target, &Default::default())
                .unwrap_err()
                .contains("disabled")
        );
        let mut progress = 0;
        assert!(
            backend
                .finalize(&target, &mut |_| progress += 1)
                .unwrap_err()
                .contains("disabled")
        );
        assert_eq!(progress, 0);
    }
}
