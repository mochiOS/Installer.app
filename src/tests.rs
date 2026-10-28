use super::*;
use appcore::consent::{ConsentRecord, DataCategory, DocumentReceipt};
use appcore::storage::{Entry, Kind, Snapshot};
use backend::{Backend, Disk, Inventory, Plan, Progress, VerifiedTarget};
use documents::Documents;
use model::{Action, Busy, Dialog, Effect, Page, Wizard};
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

fn docs() -> Documents {
    Documents::load(&documents::resource_root()).unwrap()
}
fn wizard() -> Wizard {
    Wizard::new(Ok(docs()), "test-release".into())
}
fn consent(record: &mut ConsentRecord, docs: &Documents) {
    record.terms = Some(docs.receipt("terms.txt", 1));
    record.privacy = Some(docs.receipt("privacy.txt", 2));
    record.open_source_licenses = Some(DocumentReceipt::new(
        &docs.catalog.document_version,
        &docs.license_bytes(),
        3,
    ));
    for c in DataCategory::ALL {
        record.set_permission(c, false, &docs.policy(c).version, 4);
    }
}
fn disk(id: u64, name: &str) -> Entry {
    Entry {
        id,
        parent: None,
        kind: Kind::Disk,
        name: name.into(),
        capacity: 64 * 1024 * 1024 * 1024,
        used: None,
        read_only: Some(false),
        format: None,
        mount: None,
        start_sector: 0,
        sector_bytes: 512,
        capacity_from_partition_map: false,
    }
}
fn inventory() -> Inventory {
    let mut part = disk(11, "Source partition");
    part.kind = Kind::Partition;
    part.parent = Some(10);
    let mut root = disk(12, "Source volume");
    root.kind = Kind::Volume;
    root.parent = Some(11);
    root.mount = Some("/".into());
    Inventory::from_snapshot(Snapshot {
        physical_inventory_unavailable: false,
        entries: vec![
            disk(10, "Installation media"),
            part,
            root,
            disk(20, "Target disk"),
        ],
    })
}
fn plan() -> Plan {
    Plan {
        target: disk(20, "Target disk"),
        release: "test-release".into(),
        partitions: vec!["Test-only layout".into()],
        formats: vec!["Test-only Data".into()],
        erased_data: "All test target data will be removed.".into(),
        steps: vec!["Test write and verification".into()],
        identity: "test-provider-plan".into(),
    }
}
#[test]
fn legal_files_are_exact_placeholders_and_loaded_from_external_utf8_files() {
    let docs = docs();
    assert!(docs.files.values().all(|text| text == "abcdef"));
    assert_eq!(docs.catalog.licenses.len(), 2);
    assert_eq!(docs.catalog.licenses[0].license, "Apache-2.0");
    assert_eq!(docs.catalog.licenses[1].license, "MIT");
    assert!(Documents::load(&documents::resource_root().join("missing")).is_err());
}
#[test]
fn legal_gates_and_disagreement_confirmation_cannot_be_skipped() {
    let mut w = wizard();
    w.apply(Action::Next, 1);
    assert_eq!(w.page, Page::Terms);
    assert!(w.can_next());
    w.apply(Action::Next, 2);
    assert_eq!(w.page, Page::Terms);
    assert!(matches!(w.apply(Action::ConfirmInstall, 2), Effect::None));
    w.apply(Action::DisagreeTerms, 3);
    assert_eq!(w.dialog, Some(Dialog::Disagree));
    w.apply(Action::AgreeTerms, 3);
    assert!(w.record.terms.is_none());
    assert!(matches!(w.apply(Action::Quit, 4), Effect::Quit));
    w.apply(Action::DismissDialog, 4);
    assert_eq!(w.page, Page::Terms);
    w.apply(Action::AgreeTerms, 5);
    assert_eq!(w.page, Page::Privacy);
    assert_eq!(w.record.terms.as_ref().unwrap().acknowledged_at, 5);
    w.apply(Action::ConfirmPrivacy, 6);
    assert_eq!(w.page, Page::Licenses);
    w.apply(Action::License(1), 6);
    assert_eq!(w.license, 1);
    w.apply(Action::ConfirmLicenses, 7);
    assert_eq!(w.page, Page::Collection(DataCategory::ActiveUsage));
}
#[test]
fn choices_are_independent_explicit_and_only_committed_by_continue() {
    let mut w = wizard();
    for (i, category) in DataCategory::ALL.into_iter().enumerate() {
        w.page = Page::Collection(category);
        assert!(!w.can_next());
        w.apply(Action::Next, 10);
        assert!(w.record.permission(category).is_none());
        w.apply(Action::Choose(i == 1), 11);
        assert_eq!(w.page, Page::Collection(category));
        assert!(w.record.permission(category).is_none());
        assert!(w.can_next());
        w.apply(Action::Next, 12);
        assert_eq!(w.record.permission(category).unwrap().allowed, i == 1);
    }
    assert_eq!(w.page, Page::ConsentSummary);
    assert!(!w.consent_ready());
    assert!(matches!(w.apply(Action::Next, 12), Effect::None));
}
#[test]
fn summary_edits_return_to_summary_and_preserve_unrelated_choices() {
    let mut w = wizard();
    consent(&mut w.record, &docs());
    w.page = Page::ConsentSummary;
    let original = w.record.clone();
    for page in [Page::Terms, Page::Privacy, Page::Licenses] {
        w.apply(Action::Edit(page), 20);
        assert_eq!(w.page, page);
        w.apply(
            match page {
                Page::Terms => Action::AgreeTerms,
                Page::Privacy => Action::ConfirmPrivacy,
                _ => Action::ConfirmLicenses,
            },
            21,
        );
        assert_eq!(w.page, Page::ConsentSummary);
    }
    w.apply(
        Action::Edit(Page::Collection(DataCategory::CrashReports)),
        22,
    );
    w.apply(Action::Choose(true), 23);
    assert_eq!(w.record.crash_reports, original.crash_reports);
    w.apply(Action::Next, 24);
    assert_eq!(w.page, Page::ConsentSummary);
    assert!(w.record.crash_reports.as_ref().unwrap().allowed);
    assert_eq!(w.record.active_usage, original.active_usage);
    assert_eq!(
        w.record.performance_diagnostics,
        original.performance_diagnostics
    );
    assert!(w.consent_ready());
    w.record.terms.as_mut().unwrap().sha256 = "0".repeat(64);
    assert!(!w.consent_ready());
}
#[test]
fn source_readonly_unknown_capacity_and_unidentified_media_are_not_destinations() {
    let i = inventory();
    assert!(i.target(10).is_none());
    assert!(i.target(20).is_some());
    for mode in 0..4 {
        let mut snapshot = i.snapshot.clone();
        match mode {
            0 => snapshot.entries[3].read_only = None,
            1 => snapshot.entries[3].read_only = Some(true),
            2 => snapshot.entries[3].capacity_from_partition_map = true,
            _ => snapshot.entries[2].mount = None,
        }
        assert!(Inventory::from_snapshot(snapshot).target(20).is_none());
    }
    assert!(
        backend::NativeBackend
            .prepare(i.target(20).unwrap(), "test-release")
            .is_err()
    );
}
#[test]
fn writes_require_a_valid_plan_and_a_separate_final_confirmation() {
    let mut w = wizard();
    consent(&mut w.record, &docs());
    w.inventory = Some(inventory());
    w.selected_disk = Some(20);
    w.page = Page::Review;
    assert!(matches!(w.apply(Action::RequestInstall, 20), Effect::None));
    assert_eq!(w.dialog, None);
    w.plan = Some(plan());
    w.apply(Action::RequestInstall, 21);
    assert_eq!(w.dialog, Some(Dialog::Erase));
    assert_ne!(w.page, Page::Installing);
    w.apply(Action::DismissDialog, 22);
    assert_eq!(w.dialog, None);
    assert_eq!(w.page, Page::Review);
    w.apply(Action::RequestInstall, 23);
    assert!(matches!(
        w.apply(Action::ConfirmInstall, 24),
        Effect::Install(_)
    ));
    assert_eq!(w.page, Page::Installing);
    assert_eq!(w.busy, Some(Busy::Installing));
    w.finished(Err("Test write failed".into()));
    assert_eq!(w.page, Page::Installing);
    assert!(w.error.is_some());
    assert!(w.installed.is_none());
}

static NEXT: AtomicUsize = AtomicUsize::new(1);
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "mochios-installer-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
struct TestBackend {
    root: PathBuf,
    writes: AtomicUsize,
    failure: u8,
}
impl Backend for TestBackend {
    fn inventory(&self) -> Result<Inventory, String> {
        Ok(inventory())
    }
    fn prepare(&self, _: &Disk, _: &str) -> Result<Plan, String> {
        Ok(plan())
    }
    fn revalidate(&self, _: &Plan) -> Result<(), String> {
        if self.failure == 1 {
            Err("Test revalidation failure".into())
        } else {
            Ok(())
        }
    }
    fn install(
        &self,
        _: &Plan,
        progress: &mut dyn FnMut(Progress),
    ) -> Result<VerifiedTarget, String> {
        self.writes.fetch_add(1, Ordering::Relaxed);
        progress(Progress::stage("Test write"));
        if self.failure == 2 {
            return Err("Test write failure".into());
        }
        Ok(VerifiedTarget {
            id: 20,
            release: "test-release".into(),
            mounted_root: self.root.clone(),
        })
    }
    fn finalize(&self, _: &VerifiedTarget, _: &mut dyn FnMut(Progress)) -> Result<(), String> {
        if self.failure == 3 {
            Err("Test final verification failure".into())
        } else {
            Ok(())
        }
    }
    fn reboot(&self) -> Result<(), String> {
        Err("Not supported in tests".into())
    }
}
#[test]
fn backend_errors_never_report_success_and_only_verified_target_receives_consent() {
    for failure in 0..4 {
        let root = Temp::new();
        let backend = TestBackend {
            root: root.0.clone(),
            writes: AtomicUsize::new(0),
            failure,
        };
        let mut w = wizard();
        consent(&mut w.record, &docs());
        let request = model::Authorization {
            plan: plan(),
            consent: w.record.clone(),
        };
        let result = engine::install(&backend, request, &docs(), &mut |_| {});
        assert_eq!(result.is_ok(), failure == 0);
        assert_eq!(
            backend.writes.load(Ordering::Relaxed),
            usize::from(failure != 1)
        );
        let store = appcore::consent::ConsentStore::at_directory(root.0.join("var/config/privacy"));
        if failure == 0 {
            assert_eq!(store.load().unwrap(), w.record);
        }
        w.page = Page::Installing;
        w.busy = Some(Busy::Installing);
        w.selected_disk = Some(20);
        w.finished(result);
        assert_eq!(w.page == Page::Complete, failure == 0);
    }
}
#[test]
fn incomplete_consent_never_reaches_backend_write() {
    let root = Temp::new();
    let backend = TestBackend {
        root: root.0.clone(),
        writes: AtomicUsize::new(0),
        failure: 0,
    };
    let request = model::Authorization {
        plan: plan(),
        consent: ConsentRecord::default(),
    };
    assert!(engine::install(&backend, request, &docs(), &mut |_| {}).is_err());
    assert_eq!(backend.writes.load(Ordering::Relaxed), 0);
    assert!(!root.0.join("var").exists());
}
#[test]
fn all_pages_render_in_both_themes_at_compact_and_regular_widths() {
    use appcore::viewkit::{
        draw_command::{DisplayList, DrawCommand},
        typography::TextMeasurer,
        view::PaintContext,
    };
    let core = controller::Controller::new();
    let view = ui::InstallerView::new(core.clone());
    for theme in [Theme::LIGHT, Theme::DARK] {
        for width in [360.0, 640.0, 1040.0] {
            for page in Page::ALL {
                core.model.borrow_mut().page = page;
                core.revision.set(core.revision.get() + 1);
                let mut list = DisplayList::new();
                let mut text = TextMeasurer::new();
                view.paint(
                    Rect::new(0.0, 0.0, width, 760.0),
                    &mut PaintContext::new(&mut list, &theme, &theme.typography, &mut text),
                );
                assert!(!list.commands().is_empty());
                let mut depth = 0;
                for command in list.commands() {
                    match command {
                        DrawCommand::PushClip { .. }
                        | DrawCommand::PushRoundedClip { .. }
                        | DrawCommand::PushRoundedCornersClip { .. } => depth += 1,
                        DrawCommand::PopClip => {
                            depth -= 1;
                            assert!(depth >= 0);
                        }
                        _ => {}
                    }
                }
                assert_eq!(depth, 0);
            }
        }
    }
}

#[test]
fn data_policy_versions_are_independent_and_locale_falls_back_to_english() {
    let mut documents = docs();
    let mut record = ConsentRecord::default();
    consent(&mut record, &documents);
    documents.catalog.data_policies[0].version = "purpose-v2".into();
    let mut w = Wizard::new(Ok(documents.clone()), "test-release".into());
    w.record = record;
    assert!(!w.consent_ready());
    assert!(documents.legal_matches(&w.record));
    assert_eq!(
        documents
            .disclosure(DataCategory::ActiveUsage, "unknown-locale")
            .summary_file,
        documents
            .disclosure(DataCategory::ActiveUsage, "en")
            .summary_file
    );
    w.page = Page::ConsentSummary;
    w.apply(
        Action::Edit(Page::Collection(DataCategory::ActiveUsage)),
        40,
    );
    w.apply(Action::Choose(false), 41);
    w.apply(Action::Next, 42);
    assert!(w.consent_ready());
    assert_eq!(
        w.record.active_usage.as_ref().unwrap().policy_version,
        "purpose-v2"
    );
    assert_eq!(
        w.record.crash_reports.as_ref().unwrap().policy_version,
        "placeholder-1"
    );
}
#[test]
fn consent_persistence_failure_prevents_completion() {
    let root = Temp::new();
    fs::create_dir(root.0.join("var")).unwrap();
    fs::write(root.0.join("var/config"), b"not a directory").unwrap();
    let backend = TestBackend {
        root: root.0.clone(),
        writes: AtomicUsize::new(0),
        failure: 0,
    };
    let mut w = wizard();
    consent(&mut w.record, &docs());
    let request = model::Authorization {
        plan: plan(),
        consent: w.record.clone(),
    };
    let result = engine::install(&backend, request, &docs(), &mut |_| {});
    assert!(result.is_err());
    w.page = Page::Installing;
    w.busy = Some(Busy::Installing);
    w.selected_disk = Some(20);
    w.finished(result);
    assert_eq!(w.page, Page::Installing);
    assert!(w.installed.is_none());
}
#[test]
fn closing_is_blocked_only_while_installation_is_running() {
    let core = controller::Controller::new();
    let mut app = InstallerApp { core: core.clone() };
    assert!(app.close_requested());
    core.model.borrow_mut().busy = Some(Busy::Installing);
    assert!(!app.close_requested());
    core.model.borrow_mut().busy = None;
    assert!(app.close_requested());
}

#[test]
fn full_wizard_reaches_completion_only_after_confirmed_verified_test_installation() {
    let root = Temp::new();
    let backend = TestBackend {
        root: root.0.clone(),
        writes: AtomicUsize::new(0),
        failure: 0,
    };
    let mut w = wizard();
    w.apply(Action::Next, 1);
    assert_eq!(w.page, Page::Terms);
    w.apply(Action::AgreeTerms, 2);
    assert_eq!(w.page, Page::Privacy);
    w.apply(Action::ConfirmPrivacy, 3);
    assert_eq!(w.page, Page::Licenses);
    w.apply(Action::ConfirmLicenses, 4);
    for category in DataCategory::ALL {
        assert_eq!(w.page, Page::Collection(category));
        w.apply(Action::Choose(false), 5);
        assert_eq!(w.page, Page::Collection(category));
        w.apply(Action::Next, 6);
    }
    assert_eq!(w.page, Page::ConsentSummary);
    assert!(matches!(w.apply(Action::Next, 7), Effect::Discover));
    w.discovered(backend.inventory());
    assert_eq!(w.page, Page::Destination);
    w.apply(Action::SelectDisk(20), 8);
    let Effect::Prepare(disk) = w.apply(Action::Next, 9) else {
        panic!("Expected preparation")
    };
    assert_eq!(w.page, Page::Storage);
    w.prepared(backend.prepare(&disk, &w.release));
    w.apply(Action::Next, 10);
    assert_eq!(w.page, Page::Review);
    w.apply(Action::RequestInstall, 11);
    assert_eq!(w.dialog, Some(Dialog::Erase));
    assert_eq!(backend.writes.load(Ordering::Relaxed), 0);
    assert!(!root.0.join("var").exists());
    let Effect::Install(request) = w.apply(Action::ConfirmInstall, 12) else {
        panic!("Expected explicit install authorization")
    };
    assert_eq!(w.page, Page::Installing);
    let result = engine::install(&backend, request, &docs(), &mut |_| {});
    w.finished(result);
    assert_eq!(w.page, Page::Complete);
    assert!(w.installed.is_some());
}
#[test]
fn terms_dialog_renders_its_actions_and_blocks_the_underlying_page() {
    use appcore::viewkit::{
        draw_command::{DisplayList, DrawCommand},
        typography::TextMeasurer,
        view::PaintContext,
    };
    let core = controller::Controller::new();
    core.model.borrow_mut().page = Page::Terms;
    core.model.borrow_mut().dialog = Some(Dialog::Disagree);
    let view = ui::InstallerView::new(core);
    let theme = Theme::LIGHT;
    let mut list = DisplayList::new();
    let mut text = TextMeasurer::new();
    view.paint(
        Rect::new(0.0, 0.0, 640.0, 600.0),
        &mut PaintContext::new(&mut list, &theme, &theme.typography, &mut text),
    );
    for title in ["Agreement Required", "Back to Terms", "Quit Installer"] {
        assert!(list.commands().iter().any(
            |command| matches!(command,DrawCommand::DrawText{command} if command.text==title)
        ));
    }
}

#[test]
fn mounted_data_disks_and_unresolved_mounts_are_not_destinations() {
    let mut snapshot = inventory().snapshot;
    let mut volume = disk(21, "Working files");
    volume.kind = Kind::Volume;
    volume.parent = Some(20);
    volume.mount = Some("/Volumes/Work".into());
    snapshot.entries.push(volume);
    let mounted = Inventory::from_snapshot(snapshot.clone());
    assert!(mounted.target(20).is_none());
    assert!(
        mounted
            .disks
            .iter()
            .find(|d| d.entry.id == 20)
            .unwrap()
            .blocked
            .as_ref()
            .unwrap()
            .contains("mounted volume")
    );
    snapshot.entries.last_mut().unwrap().parent = Some(999);
    assert!(Inventory::from_snapshot(snapshot).target(20).is_none());
}
