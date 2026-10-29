//! UI commands and bounded asynchronous jobs. No service I/O in painting.
use crate::backend::{Backend, NativeBackend, Plan, Progress};
use crate::documents::{self, Documents};
use crate::model::{Action, Busy, Effect, Receipt, Wizard};
use appcore::prelude::*;
use appcore::task::{
    Progress as TaskProgress, TaskContext, TaskError, TaskResult, TaskScope, TaskSlot,
    TaskStartError,
};
use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, VecDeque};
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

enum OperationResult {
    Prepared(Plan),
    Installed(Receipt),
    Reboot,
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
    backend: Arc<dyn Backend>,
    owner: Weak<Self>,
    tasks: Result<TaskScope, TaskStartError>,
    discovery_slot: RefCell<TaskSlot>,
}
impl Controller {
    pub fn new() -> Rc<Self> {
        let docs = Documents::load(&documents::resource_root())
            .map_err(|e| format!("Legal documents could not be loaded: {e}"));
        let license_count = docs.as_ref().map_or(0, |d| d.catalog.licenses.len());
        Rc::new_cyclic(|owner| Self {
            model: RefCell::new(Wizard::new(docs, env!("MOCHIOS_VERSION").into())),
            revision: Cell::new(0),
            icon: ImageData::decode(include_bytes!("../appicon.png")).ok(),
            scrolls: (0..13).map(|_| ScrollState::new()).collect(),
            license_list_scroll: ScrollState::new(),
            license_detail_scrolls: (0..license_count).map(|_| ScrollState::new()).collect(),
            queue: Rc::new(RefCell::new(VecDeque::new())),
            buttons: RefCell::new(BTreeMap::new()),
            backend: Arc::new(NativeBackend::default()),
            owner: owner.clone(),
            tasks: TaskScope::new(),
            discovery_slot: RefCell::new(TaskSlot::new()),
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
        if matches!(effect, Effect::Discover) {
            let result = self
                .tasks
                .as_ref()
                .map_err(|error| *error)
                .and_then(|scope| {
                    let backend = self.backend.clone();
                    let owner = self.owner.clone();
                    self.discovery_slot
                        .borrow_mut()
                        .spawn(
                            scope,
                            move |task| {
                                task.check_cancelled()?;
                                backend.inventory().map_err(TaskError::Operation)
                            },
                            move |result| {
                                if let Some(core) = owner.upgrade() {
                                    core.model.borrow_mut().discovered(result.map_err(|error| {
                                        format!("Disk discovery could not finish: {error}")
                                    }));
                                    core.changed();
                                }
                            },
                        )
                        .map(|_| ())
                });
            if let Err(error) = result {
                let mut model = self.model.borrow_mut();
                model.busy = None;
                model.error = Some(format!("Disk discovery could not start: {error}"));
            }
            return;
        }
        let backend = self.backend.clone();
        let release = self.model.borrow().release.clone();
        let documents = self.model.borrow().documents.as_ref().ok().cloned();
        let progress_owner = self.owner.clone();
        let completion_owner = self.owner.clone();
        let result = self
            .tasks
            .as_ref()
            .map_err(|error| *error)
            .and_then(|scope| {
                scope
                    .spawn_with_progress(
                        move |task| {
                            run_operation(
                                effect,
                                backend.as_ref(),
                                &release,
                                documents.as_deref(),
                                &task,
                            )
                        },
                        move |progress| {
                            if let Some(core) = progress_owner.upgrade() {
                                if core.model.borrow().busy.is_some() {
                                    core.model.borrow_mut().progress = Some(Progress {
                                        stage: progress.message().into(),
                                        completed: progress.completed(),
                                        total: progress.total(),
                                    });
                                    core.changed();
                                }
                            }
                        },
                        move |result| {
                            if let Some(core) = completion_owner.upgrade() {
                                core.completed(result);
                            }
                        },
                    )
                    .map(|_| ())
            });
        if let Err(error) = result {
            self.failed(format!("The operation could not start: {error}"));
        }
    }
    fn failed(&self, error: String) {
        let mut model = self.model.borrow_mut();
        model.busy = None;
        model.progress = None;
        model.error = Some(error);
        self.changed();
    }
    fn completed(&self, result: TaskResult<OperationResult, String>) {
        match result {
            Ok(OperationResult::Prepared(plan)) => self.model.borrow_mut().prepared(Ok(plan)),
            Ok(OperationResult::Installed(receipt)) => {
                self.model.borrow_mut().finished(Ok(receipt))
            }
            Ok(OperationResult::Reboot) => {
                let mut model = self.model.borrow_mut();
                if model.busy == Some(Busy::Rebooting) {
                    model.busy = None;
                    model.error = None;
                }
            }
            Err(error) => self.failed(task_error(error)),
        }
        self.changed();
    }
    /// 書き込み・再起動の実行中はfalseを返し、ウィンドウ終了を保留します。
    /// 読み取り専用の検証中は終了できます。Scope破棄後の通知はAppCoreが解除します。
    pub fn can_close(&self) -> bool {
        !matches!(
            self.model.borrow().busy,
            Some(Busy::Installing | Busy::Rebooting)
        )
    }
}

// Only this worker boundary performs blocking calls. No UI state crosses it.
fn run_operation(
    effect: Effect,
    backend: &dyn Backend,
    release: &str,
    documents: Option<&Documents>,
    task: &TaskContext,
) -> TaskResult<OperationResult, String> {
    task.check_cancelled()?;
    match effect {
        Effect::Prepare(disk) => {
            task.report(TaskProgress::indeterminate(
                "Checking storage configuration…",
            ));
            let result = backend.prepare(&disk, release);
            task.check_cancelled()?;
            result
                .map(OperationResult::Prepared)
                .map_err(TaskError::Operation)
        }
        Effect::Install(request) => {
            // Once installation starts there are no cancellation checkpoints:
            // an arbitrary abort could leave partially committed disk state.
            let result = documents
                .ok_or_else(|| "Legal documents are unavailable.".into())
                .and_then(|docs| {
                    crate::engine::install(backend, request, docs, &mut |progress| {
                        task.report(task_progress(progress));
                    })
                });
            result
                .map(OperationResult::Installed)
                .map_err(TaskError::Operation)
        }
        Effect::Reboot => backend
            .reboot()
            .map(|()| OperationResult::Reboot)
            .map_err(TaskError::Operation),
        _ => Err(TaskError::Operation(
            "An unsupported background operation was requested.".into(),
        )),
    }
}
fn task_progress(progress: Progress) -> TaskProgress {
    if let (Some(done), Some(total)) = (progress.completed, progress.total) {
        if let Ok(value) = TaskProgress::measured(&progress.stage, done, total) {
            return value;
        }
    }
    TaskProgress::indeterminate(progress.stage)
}
fn task_error(error: TaskError<String>) -> String {
    match error {
        TaskError::Operation(error) => error,
        TaskError::Cancelled => "The operation was cancelled before completion.".into(),
        TaskError::WorkerStopped => {
            "The operation worker stopped unexpectedly. Completion has not been verified.".into()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::{Disk, Inventory, VerifiedTarget};
    use crate::model::Page;
    use crate::tests::{consent, docs, inventory, plan};
    use appcore::task::{TaskExecutor, TaskStatus};
    use std::sync::{Mutex, mpsc};
    use std::time::{Duration, Instant};

    struct ReadOnlyBackend {
        started: mpsc::Sender<std::thread::ThreadId>,
        release: Mutex<mpsc::Receiver<()>>,
    }
    impl Backend for ReadOnlyBackend {
        fn inventory(&self) -> Result<Inventory, String> {
            Ok(inventory())
        }
        fn prepare(&self, _: &Disk, _: &str) -> Result<Plan, String> {
            self.started.send(std::thread::current().id()).unwrap();
            self.release
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(5))
                .unwrap();
            Ok(plan())
        }
        fn revalidate(&self, _: &Plan) -> Result<(), String> {
            panic!("read-only check must not install")
        }
        fn install(&self, _: &Plan, _: &mut dyn FnMut(Progress)) -> Result<VerifiedTarget, String> {
            panic!("read-only check must not write")
        }
        fn finalize(&self, _: &VerifiedTarget, _: &mut dyn FnMut(Progress)) -> Result<(), String> {
            panic!("read-only check must not finalize")
        }
        fn reboot(&self) -> Result<(), String> {
            Err("Restart is unavailable in this environment.".into())
        }
    }
    fn preparing() -> Rc<Controller> {
        let core = Controller::new();
        let mut model = core.model.borrow_mut();
        model.release = "test-release".into();
        consent(&mut model.record, &docs());
        model.inventory = Some(inventory());
        model.selected_disk = Some(20);
        model.page = Page::Storage;
        model.busy = Some(Busy::Preparing);
        model.progress = Some(Progress::stage("Checking storage configuration…"));
        drop(model);
        core
    }
    fn wait_for(handle: &appcore::task::TaskHandle) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !handle.status().is_finished() {
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
    }
    #[test]
    fn storage_check_runs_off_ui_and_returns_a_plan_without_writes() {
        let core = preparing();
        let (started, ready) = mpsc::channel();
        let (release, waiting) = mpsc::channel();
        let backend = ReadOnlyBackend {
            started,
            release: Mutex::new(waiting),
        };
        let disk = inventory().target(20).unwrap().clone();
        let scope = TaskScope::with_executor(&TaskExecutor::new(1, 2).unwrap()).unwrap();
        let (completed, result) = mpsc::channel();
        let handle = scope
            .spawn(
                move |task| -> TaskResult<(), String> {
                    completed
                        .send(run_operation(
                            Effect::Prepare(disk),
                            &backend,
                            "test-release",
                            None,
                            &task,
                        ))
                        .unwrap();
                    Ok(())
                },
                |_| {},
            )
            .unwrap();
        assert_ne!(
            ready.recv_timeout(Duration::from_secs(5)).unwrap(),
            std::thread::current().id()
        );
        assert!(core.can_close());
        assert!(!core.model.borrow().can_next());
        release.send(()).unwrap();
        // Deliver the actual worker outcome through the controller's UI boundary.
        core.completed(result.recv_timeout(Duration::from_secs(5)).unwrap());
        wait_for(&handle);
        let model = core.model.borrow();
        assert_eq!(model.plan, Some(plan()));
        assert!(model.busy.is_none());
        assert!(model.progress.is_none());
        assert!(model.can_next());
        assert!(model.installed.is_none());
    }
    #[test]
    fn cancelling_read_only_check_does_not_publish_its_finished_plan() {
        let core = preparing();
        let (started, ready) = mpsc::channel();
        let (release, waiting) = mpsc::channel();
        let backend = ReadOnlyBackend {
            started,
            release: Mutex::new(waiting),
        };
        let disk = inventory().target(20).unwrap().clone();
        let scope = TaskScope::with_executor(&TaskExecutor::new(1, 2).unwrap()).unwrap();
        let (completed, result) = mpsc::channel();
        let handle = scope
            .spawn(
                move |task| {
                    let outcome =
                        run_operation(Effect::Prepare(disk), &backend, "test-release", None, &task);
                    completed
                        .send(matches!(outcome, Err(TaskError::Cancelled)))
                        .unwrap();
                    outcome
                },
                |_| {},
            )
            .unwrap();
        ready.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(handle.cancel());
        assert_eq!(handle.status(), TaskStatus::CancellationRequested);
        release.send(()).unwrap();
        assert!(result.recv_timeout(Duration::from_secs(5)).unwrap());
        wait_for(&handle);
        assert_eq!(handle.status(), TaskStatus::Cancelled);
        core.completed(Err(TaskError::Cancelled));
        assert!(core.model.borrow().plan.is_none());
        assert!(!core.model.borrow().can_next());
        assert!(core.can_close());
    }
    #[test]
    fn failed_submission_clears_busy_and_never_provides_a_plan() {
        let mut core = Rc::try_unwrap(preparing()).ok().unwrap();
        core.tasks = Err(TaskStartError::WorkerUnavailable);
        core.execute(Effect::Prepare(inventory().target(20).unwrap().clone()));
        let model = core.model.borrow();
        assert!(model.busy.is_none());
        assert!(model.progress.is_none());
        assert!(model.plan.is_none());
        assert!(model.error.as_ref().unwrap().contains("could not start"));
        assert!(!model.can_next());
    }
    #[test]
    fn failures_and_worker_stop_never_display_installation_complete() {
        for error in [
            TaskError::Operation("Device disconnected".into()),
            TaskError::WorkerStopped,
        ] {
            let core = preparing();
            core.model.borrow_mut().page = Page::Installing;
            core.model.borrow_mut().busy = Some(Busy::Installing);
            assert!(!core.can_close());
            core.completed(Err(error));
            let model = core.model.borrow();
            assert_eq!(model.page, Page::Installing);
            assert!(model.installed.is_none());
            assert!(model.busy.is_none());
            assert!(model.error.is_some());
            assert!(core.can_close());
        }
    }
    #[test]
    fn invalid_progress_is_indeterminate_and_full_progress_is_not_success() {
        for (done, total) in [(1, 0), (3, 2)] {
            let progress = task_progress(Progress {
                stage: "Verifying".into(),
                completed: Some(done),
                total: Some(total),
            });
            assert_eq!(progress.message(), "Verifying");
            assert_eq!(progress.fraction(), None);
        }
        let progress = task_progress(Progress {
            stage: "Verifying".into(),
            completed: Some(2),
            total: Some(2),
        });
        assert_eq!(progress.fraction(), Some(1.0));
        let core = preparing();
        core.model.borrow_mut().busy = Some(Busy::Installing);
        assert!(!core.can_close());
        assert!(core.model.borrow().installed.is_none());
    }
}
