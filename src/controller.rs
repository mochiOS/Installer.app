//! UI commands and bounded asynchronous jobs. No service I/O in painting.
use crate::backend::{Backend, Inventory, NativeBackend, Plan, Progress};
use crate::documents::{self, Documents};
use crate::model::{Action, Busy, Effect, Receipt, Wizard};
use appcore::prelude::*;
use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, VecDeque};
use std::rc::Rc;
use std::sync::{Arc, mpsc};
use std::time::{SystemTime, UNIX_EPOCH};

enum JobEvent {
    Inventory(Result<Inventory, String>),
    Prepared(Result<Plan, String>),
    Progress(Progress),
    Installed(Result<Receipt, String>),
    Reboot(Result<(), String>),
}
pub struct Controller {
    pub model: RefCell<Wizard>,
    pub revision: Cell<u64>,
    pub icon: Option<ImageData>,
    pub scrolls: Vec<ScrollState>,
    pub license_list_scroll: ScrollState,
    pub license_detail_scrolls: Vec<ScrollState>,
    queue: Rc<RefCell<VecDeque<Action>>>,
    buttons: RefCell<BTreeMap<String, ButtonInteractionState>>,
    worker: RefCell<Option<mpsc::Receiver<JobEvent>>>,
    backend: Arc<dyn Backend>,
}
impl Controller {
    pub fn new() -> Rc<Self> {
        let docs = Documents::load(&documents::resource_root())
            .map_err(|e| format!("Legal documents could not be loaded: {e}"));
        let license_count = docs.as_ref().map_or(0, |d| d.catalog.licenses.len());
        Rc::new(Self {
            model: RefCell::new(Wizard::new(docs, env!("MOCHIOS_VERSION").into())),
            revision: Cell::new(0),
            icon: ImageData::decode(include_bytes!("../appicon.png")).ok(),
            scrolls: (0..13).map(|_| ScrollState::new()).collect(),
            license_list_scroll: ScrollState::new(),
            license_detail_scrolls: (0..license_count).map(|_| ScrollState::new()).collect(),
            queue: Rc::new(RefCell::new(VecDeque::new())),
            buttons: RefCell::new(BTreeMap::new()),
            worker: RefCell::new(None),
            backend: Arc::new(NativeBackend),
        })
    }
    pub fn button(&self, label: &str, action: Action) -> Button {
        let key = format!("{action:?}");
        let interaction = self.buttons.borrow_mut().entry(key).or_default().clone();
        let queue = self.queue.clone();
        Button::with_interaction_and_label(interaction, label)
            .on_click(move || queue.borrow_mut().push_back(action.clone()))
    }
    pub fn row(&self, title: &str, subtitle: String, selected: bool, action: Action) -> ListRow {
        let key = format!("{action:?}");
        let interaction = self.buttons.borrow_mut().entry(key).or_default().clone();
        let queue = self.queue.clone();
        ListRow::with_interaction(interaction, title)
            .subtitle(subtitle)
            .selected(selected)
            .on_select(move || queue.borrow_mut().push_back(action.clone()))
    }
    pub fn supports_reboot(&self) -> bool {
        self.backend.supports_reboot()
    }
    pub fn submit(&self, action: Action) {
        self.queue.borrow_mut().push_back(action);
    }
    fn changed(&self) {
        self.revision.set(self.revision.get().wrapping_add(1));
    }
    pub fn flush(&self) -> bool {
        let actions: Vec<_> = self.queue.borrow_mut().drain(..).collect();
        if actions.is_empty() {
            return false;
        }
        for action in actions {
            let needs_timestamp = matches!(
                action,
                Action::AgreeTerms | Action::ConfirmPrivacy | Action::ConfirmLicenses
            ) || matches!(action, Action::Next)
                && matches!(self.model.borrow().page, crate::model::Page::Collection(_));
            let now = match SystemTime::now().duration_since(UNIX_EPOCH) {
                Ok(time) if time.as_secs() > 0 => time.as_secs(),
                _ if !needs_timestamp => 0,
                _ => {
                    self.model.borrow_mut().error =
                        Some("The system clock is unavailable. Consent cannot be recorded.".into());
                    self.changed();
                    continue;
                }
            };
            let effect = self.model.borrow_mut().apply(action, now);
            self.execute(effect);
            self.changed();
        }
        true
    }
    fn execute(&self, effect: Effect) {
        if matches!(effect, Effect::None) {
            return;
        }
        if matches!(effect, Effect::Quit) {
            request_exit();
            return;
        }
        let (sender, receiver) = mpsc::sync_channel(32);
        let backend = self.backend.clone();
        let release = self.model.borrow().release.clone();
        let documents = self.model.borrow().documents.as_ref().ok().cloned();
        let result = std::thread::Builder::new()
            .name("installer-operation".into())
            .spawn(move || {
                let event = match effect {
                    Effect::Discover => JobEvent::Inventory(backend.inventory()),
                    Effect::Prepare(disk) => JobEvent::Prepared(backend.prepare(&disk, &release)),
                    Effect::Install(request) => {
                        let result = documents
                            .ok_or_else(|| "Legal documents are unavailable.".into())
                            .and_then(|docs| {
                                crate::engine::install(
                                    backend.as_ref(),
                                    request,
                                    &docs,
                                    &mut |progress| {
                                        let _ = sender.try_send(JobEvent::Progress(progress));
                                    },
                                )
                            });
                        JobEvent::Installed(result)
                    }
                    Effect::Reboot => JobEvent::Reboot(backend.reboot()),
                    _ => return,
                };
                let _ = sender.send(event);
            });
        match result {
            Ok(_) => *self.worker.borrow_mut() = Some(receiver),
            Err(e) => {
                let mut model = self.model.borrow_mut();
                model.busy = None;
                model.error = Some(format!("The operation could not start: {e}"));
            }
        }
    }
    pub fn poll(&self) -> bool {
        let mut worker_failed = false;
        let events = {
            let worker = self.worker.borrow();
            let Some(receiver) = worker.as_ref() else {
                return false;
            };
            let mut events = Vec::new();
            loop {
                match receiver.try_recv() {
                    Ok(event) => events.push(event),
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        if events.is_empty() && self.model.borrow().busy.is_some() {
                            worker_failed = true;
                            let mut model = self.model.borrow_mut();
                            model.busy = None;
                            model.error = Some("The installation worker stopped unexpectedly. Completion has not been verified.".into());
                        }
                        break;
                    }
                }
            }
            events
        };
        let changed = worker_failed || !events.is_empty();
        for event in events {
            let mut model = self.model.borrow_mut();
            match event {
                JobEvent::Inventory(result) => model.discovered(result),
                JobEvent::Prepared(result) => model.prepared(result),
                JobEvent::Progress(progress) if model.busy == Some(Busy::Installing) => {
                    model.progress = Some(progress)
                }
                JobEvent::Installed(result) => model.finished(result),
                JobEvent::Reboot(result) => {
                    model.busy = None;
                    if let Err(e) = result {
                        model.error = Some(e);
                    }
                }
                _ => {}
            }
        }
        if self.model.borrow().busy.is_none() {
            self.worker.borrow_mut().take();
        }
        if changed {
            self.changed();
        }
        changed
    }
}
