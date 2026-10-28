//! Worker-only installation orchestration. Authorization and target identity
//! are rechecked before invoking the backend's first destructive operation.
use crate::backend::{Backend, Progress};
use crate::documents::Documents;
use crate::model::{Authorization, Receipt};
use appcore::consent::{ConsentStore, DataCategory};

pub fn install(
    backend: &dyn Backend,
    request: Authorization,
    docs: &Documents,
    progress: &mut dyn FnMut(Progress),
) -> Result<Receipt, String> {
    if !request.consent.complete()
        || !docs.legal_matches(&request.consent)
        || !DataCategory::ALL.into_iter().all(|c| {
            request
                .consent
                .permission(c)
                .is_some_and(|p| p.policy_version == docs.policy(c).version)
        })
    {
        return Err("Required legal confirmations or data choices are missing or outdated.".into());
    }
    progress(Progress::stage("Rechecking the installation destination…"));
    let inventory = backend.inventory()?;
    if !inventory
        .target(request.plan.target.id)
        .is_some_and(|d| d.entry == request.plan.target)
    {
        return Err("The installation destination changed or is no longer safe to use.".into());
    }
    backend.revalidate(&request.plan)?;
    let target = backend.install(&request.plan, progress)?;
    if target.id != request.plan.target.id
        || target.release != request.plan.release
        || !target.mounted_root.is_absolute()
        || target.mounted_root.parent().is_none()
    {
        return Err("The installation provider did not verify the expected destination.".into());
    }
    progress(Progress::stage("Saving device privacy settings…"));
    let store = ConsentStore::at_directory(target.mounted_root.join("var/config/privacy"));
    store
        .save(&request.consent)
        .map_err(|e| format!("Device privacy settings could not be saved: {e}"))?;
    if store
        .load()
        .map_err(|e| format!("Device privacy settings could not be verified: {e}"))?
        != request.consent
    {
        return Err("Saved device privacy settings do not match the confirmed choices.".into());
    }
    backend.finalize(&target, progress)?;
    Ok(Receipt {
        target_id: target.id,
        release: target.release,
        consent: request.consent,
    })
}
