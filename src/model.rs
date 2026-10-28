//! Pure wizard state; neither rendering nor navigation can write a disk.
use crate::backend::{Disk, Inventory, Plan, Progress};
use crate::documents::Documents;
use appcore::consent::{ConsentRecord, DataCategory, DocumentReceipt};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    Welcome,
    Terms,
    Privacy,
    Licenses,
    Collection(DataCategory),
    ConsentSummary,
    Destination,
    Storage,
    Review,
    Installing,
    Complete,
}
impl Page {
    pub const ALL: [Self; 13] = [
        Self::Welcome,
        Self::Terms,
        Self::Privacy,
        Self::Licenses,
        Self::Collection(DataCategory::ActiveUsage),
        Self::Collection(DataCategory::CrashReports),
        Self::Collection(DataCategory::PerformanceDiagnostics),
        Self::ConsentSummary,
        Self::Destination,
        Self::Storage,
        Self::Review,
        Self::Installing,
        Self::Complete,
    ];
    pub fn index(self) -> usize {
        Self::ALL.iter().position(|p| *p == self).unwrap()
    }
    pub fn title(self) -> &'static str {
        match self {
            Self::Welcome => "Welcome",
            Self::Terms => "Terms of Use",
            Self::Privacy => "Privacy Policy",
            Self::Licenses => "Open Source Licenses",
            Self::Collection(DataCategory::ActiveUsage) => "Active Usage Statistics",
            Self::Collection(DataCategory::CrashReports) => "Crash Reports",
            Self::Collection(DataCategory::PerformanceDiagnostics) => "Performance Diagnostics",
            Self::ConsentSummary => "Privacy & Consent Summary",
            Self::Destination => "Installation Destination",
            Self::Storage => "Storage Configuration",
            Self::Review => "Installation Review",
            Self::Installing => "Installing",
            Self::Complete => "Installation Complete",
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dialog {
    Disagree,
    Erase,
}
#[derive(Clone, Debug)]
pub enum Action {
    Next,
    Back,
    AgreeTerms,
    DisagreeTerms,
    ConfirmPrivacy,
    ConfirmLicenses,
    Choose(bool),
    Details,
    Edit(Page),
    License(usize),
    Refresh,
    SelectDisk(u64),
    RequestInstall,
    ConfirmInstall,
    DismissDialog,
    Quit,
    Reboot,
    Recover,
}
#[derive(Clone, Debug)]
pub enum Effect {
    None,
    Quit,
    Discover,
    Prepare(Disk),
    Install(Authorization),
    Reboot,
}
#[derive(Clone, Debug)]
pub struct Authorization {
    pub plan: Plan,
    pub consent: ConsentRecord,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Busy {
    Discovering,
    Preparing,
    Installing,
    Rebooting,
}
#[derive(Clone, Debug)]
pub struct Receipt {
    pub target_id: u64,
    pub release: String,
    pub consent: ConsentRecord,
}

pub struct Wizard {
    pub page: Page,
    pub documents: Result<Arc<Documents>, String>,
    pub record: ConsentRecord,
    pub drafts: [Option<bool>; 3],
    pub details: [bool; 3],
    pub editing: bool,
    pub license: usize,
    pub dialog: Option<Dialog>,
    pub inventory: Option<Inventory>,
    pub selected_disk: Option<u64>,
    pub plan: Option<Plan>,
    pub busy: Option<Busy>,
    pub error: Option<String>,
    pub progress: Option<Progress>,
    pub installed: Option<Receipt>,
    pub release: String,
}
impl Wizard {
    pub fn new(documents: Result<Documents, String>, release: String) -> Self {
        Self {
            page: Page::Welcome,
            documents: documents.map(Arc::new),
            record: ConsentRecord::default(),
            drafts: [None; 3],
            details: [false; 3],
            editing: false,
            license: 0,
            dialog: None,
            inventory: None,
            selected_disk: None,
            plan: None,
            busy: None,
            error: None,
            progress: None,
            installed: None,
            release,
        }
    }
    pub fn consent_ready(&self) -> bool {
        self.record.complete()
            && self.documents.as_ref().is_ok_and(|d| {
                d.legal_matches(&self.record)
                    && DataCategory::ALL.into_iter().all(|c| {
                        self.record
                            .permission(c)
                            .is_some_and(|p| p.policy_version == d.policy(c).version)
                    })
            })
    }
    pub fn target(&self) -> Option<&Disk> {
        self.selected_disk
            .and_then(|id| self.inventory.as_ref()?.target(id))
    }
    pub fn can_next(&self) -> bool {
        if self.busy.is_some() || self.dialog.is_some() {
            return false;
        }
        match self.page {
            Page::Welcome | Page::Terms | Page::Privacy | Page::Licenses => self.documents.is_ok(),
            Page::Collection(c) => self.drafts[c.index()].is_some() && self.documents.is_ok(),
            Page::ConsentSummary => self.consent_ready(),
            Page::Destination => self.consent_ready() && self.target().is_some(),
            Page::Storage => self.consent_ready() && self.target().is_some(),
            _ => false,
        }
    }
    fn acknowledged(&mut self, next: Page) {
        self.page = if self.editing {
            self.editing = false;
            Page::ConsentSummary
        } else {
            next
        };
        self.error = None;
    }
    pub fn apply(&mut self, action: Action, now: u64) -> Effect {
        if self.busy.is_some() {
            return Effect::None;
        }
        if self.dialog.is_some()
            && !matches!(
                action,
                Action::DismissDialog | Action::ConfirmInstall | Action::Quit
            )
        {
            return Effect::None;
        }
        match action {
            Action::Quit if self.dialog == Some(Dialog::Disagree) => return Effect::Quit,
            Action::DismissDialog => self.dialog = None,
            Action::DisagreeTerms if self.page == Page::Terms => {
                self.dialog = Some(Dialog::Disagree)
            }
            Action::AgreeTerms if self.page == Page::Terms => {
                if let Ok(docs) = &self.documents {
                    self.record.terms = Some(docs.receipt("terms.txt", now));
                    self.acknowledged(Page::Privacy);
                }
            }
            Action::ConfirmPrivacy if self.page == Page::Privacy => {
                if let Ok(docs) = &self.documents {
                    self.record.privacy = Some(docs.receipt("privacy.txt", now));
                    self.acknowledged(Page::Licenses);
                }
            }
            Action::ConfirmLicenses if self.page == Page::Licenses => {
                if let Ok(docs) = &self.documents {
                    self.record.open_source_licenses = Some(DocumentReceipt::new(
                        &docs.catalog.document_version,
                        &docs.license_bytes(),
                        now,
                    ));
                    self.acknowledged(Page::Collection(DataCategory::ActiveUsage));
                }
            }
            Action::Choose(allowed) => {
                if let Page::Collection(c) = self.page {
                    self.drafts[c.index()] = Some(allowed);
                }
            }
            Action::Details => {
                if let Page::Collection(c) = self.page {
                    self.details[c.index()] = !self.details[c.index()];
                }
            }
            Action::License(index) if self.page == Page::Licenses => {
                if self
                    .documents
                    .as_ref()
                    .is_ok_and(|d| index < d.catalog.licenses.len())
                {
                    self.license = index;
                }
            }
            Action::Edit(page) if self.page == Page::ConsentSummary => {
                if matches!(
                    page,
                    Page::Terms | Page::Privacy | Page::Licenses | Page::Collection(_)
                ) {
                    if let Page::Collection(c) = page {
                        self.drafts[c.index()] = self.record.permission(c).map(|p| p.allowed);
                    }
                    self.page = page;
                    self.editing = true;
                    self.error = None;
                }
            }
            Action::Next if self.can_next() => match self.page {
                Page::Welcome => self.page = Page::Terms,
                Page::Collection(category) => {
                    let docs = self.documents.as_ref().unwrap();
                    self.record.set_permission(
                        category,
                        self.drafts[category.index()].unwrap(),
                        &docs.policy(category).version,
                        now,
                    );
                    let next = Page::ALL[self.page.index() + 1];
                    self.acknowledged(next);
                }
                Page::ConsentSummary => {
                    self.selected_disk = None;
                    self.inventory = None;
                    self.plan = None;
                    self.page = Page::Destination;
                    self.busy = Some(Busy::Discovering);
                    self.error = None;
                    return Effect::Discover;
                }
                Page::Destination => {
                    if let Some(disk) = self.target().cloned() {
                        self.page = Page::Storage;
                        self.busy = Some(Busy::Preparing);
                        self.error = None;
                        self.plan = None;
                        return Effect::Prepare(disk);
                    }
                }
                Page::Storage => {
                    self.page = Page::Review;
                    self.error = None;
                }
                _ => {}
            },
            Action::Back
                if self.page.index() > 0
                    && !matches!(self.page, Page::Installing | Page::Complete) =>
            {
                self.page = if self.editing {
                    self.editing = false;
                    Page::ConsentSummary
                } else {
                    Page::ALL[self.page.index() - 1]
                };
                self.error = None;
            }
            Action::Refresh if self.page == Page::Destination => {
                self.selected_disk = None;
                self.inventory = None;
                self.plan = None;
                self.busy = Some(Busy::Discovering);
                self.error = None;
                return Effect::Discover;
            }
            Action::SelectDisk(id) if self.page == Page::Destination => {
                if self.inventory.as_ref().and_then(|i| i.target(id)).is_some() {
                    self.selected_disk = Some(id);
                    self.plan = None;
                    self.error = None;
                }
            }
            Action::RequestInstall
                if self.page == Page::Review
                    && self.consent_ready()
                    && self.plan.is_some()
                    && self.target().is_some() =>
            {
                self.dialog = Some(Dialog::Erase)
            }
            Action::ConfirmInstall
                if self.dialog == Some(Dialog::Erase)
                    && self.page == Page::Review
                    && self.consent_ready() =>
            {
                if let (Some(plan), Some(target)) = (&self.plan, self.target()) {
                    if plan.target == target.entry && plan.release == self.release {
                        let authorization = Authorization {
                            plan: plan.clone(),
                            consent: self.record.clone(),
                        };
                        self.dialog = None;
                        self.page = Page::Installing;
                        self.busy = Some(Busy::Installing);
                        self.error = None;
                        self.progress =
                            Some(Progress::stage("Checking installation authorization…"));
                        return Effect::Install(authorization);
                    }
                }
                self.dialog = None;
                self.error = Some("The installation destination changed. Review it again.".into());
            }
            Action::Recover if self.page == Page::Installing && self.error.is_some() => {
                self.page = Page::Destination;
                self.error = None;
                self.plan = None;
                self.selected_disk = None;
                self.inventory = None;
                self.busy = Some(Busy::Discovering);
                return Effect::Discover;
            }
            Action::Reboot if self.page == Page::Complete && self.installed.is_some() => {
                self.busy = Some(Busy::Rebooting);
                return Effect::Reboot;
            }
            _ => {}
        }
        Effect::None
    }
    pub fn discovered(&mut self, result: Result<Inventory, String>) {
        if self.busy != Some(Busy::Discovering) {
            return;
        }
        self.busy = None;
        match result {
            Ok(inventory) => self.inventory = Some(inventory),
            Err(error) => self.error = Some(error),
        }
    }
    pub fn prepared(&mut self, result: Result<Plan, String>) {
        if self.busy != Some(Busy::Preparing) {
            return;
        }
        self.busy = None;
        match result {
            Ok(plan)
                if self.target().is_some_and(|d| d.entry == plan.target)
                    && plan.release == self.release
                    && !plan.identity.is_empty()
                    && !plan.partitions.is_empty()
                    && !plan.steps.is_empty() =>
            {
                self.plan = Some(plan)
            }
            Ok(_) => {
                self.error = Some("The installation provider returned an invalid plan.".into())
            }
            Err(error) => self.error = Some(error),
        }
    }
    pub fn finished(&mut self, result: Result<Receipt, String>) {
        if self.busy != Some(Busy::Installing) || self.page != Page::Installing {
            return;
        }
        self.busy = None;
        match result {
            Ok(receipt)
                if Some(receipt.target_id) == self.selected_disk
                    && receipt.release == self.release
                    && receipt.consent == self.record
                    && self.consent_ready() =>
            {
                self.installed = Some(receipt);
                self.page = Page::Complete;
                self.progress = None;
            }
            Ok(_) => self.error = Some("Installation completion could not be verified.".into()),
            Err(error) => self.error = Some(error),
        }
    }
}
