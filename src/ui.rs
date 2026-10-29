use crate::backend::PROVIDER_GAP;
use crate::controller::Controller;
use crate::model::{Action, Busy, Dialog as Modal, Page};
use mochios_device_consent::DataCategory;
use appcore::prelude::*;
use appcore::viewkit::{
    event::{EventContext, EventResult, ViewEvent},
    view::{Constraints, MeasureContext, PaintContext},
};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

struct Cached {
    revision: u64,
    wide: bool,
    compact: bool,
    theme: Theme,
    view: Box<dyn View>,
}
pub struct InstallerView {
    core: Rc<Controller>,
    cached: RefCell<Option<Cached>>,
    theme: Cell<Theme>,
}
impl InstallerView {
    pub fn new(core: Rc<Controller>) -> Self {
        Self {
            core,
            cached: RefCell::new(None),
            theme: Cell::new(Theme::current()),
        }
    }
    fn paragraph(text: impl Into<String>) -> Text {
        Text::body(text.into())
    }
    fn metadata(&self, text: impl Into<String>) -> Text {
        Text::metadata(text.into()).color(self.theme.get().colors.text_secondary)
    }
    fn stack() -> VStack {
        VStack::new()
            .alignment(StackAlignment::Stretch)
            .gap(StackGap::Medium)
    }
    fn heading(&self, title: &str, subtitle: &str) -> VStack {
        Self::stack()
            .child(Text::styled(title, TextRole::TitleLarge))
            .child(Self::paragraph(subtitle).color(self.theme.get().colors.text_secondary))
    }
    fn info(&self, title: &str, value: impl Into<String>) -> VStack {
        VStack::new()
            .alignment(StackAlignment::Stretch)
            .gap(StackGap::ExtraSmall)
            .child(Text::label(title))
            .child(self.metadata(value))
    }
    fn page(&self, compact: bool) -> Box<dyn View> {
        let model = self.core.model.borrow();
        let core = &self.core;
        let Some(docs) = model.documents.as_ref().ok() else {
            return Box::new(
                self.heading(
                    "Installer resources are unavailable",
                    "The required legal documents could not be loaded.",
                )
                .child(Self::paragraph(
                    model.documents.as_ref().unwrap_err().clone(),
                )),
            );
        };
        match model.page {
            Page::Welcome => {
                let mut content = Self::stack().gap(StackGap::Large);
                if let Some(icon) = &core.icon { content = content.child(Image::new(icon.clone()).accessibility_label("Installer").frame(self.theme.get().spacing.giant, self.theme.get().spacing.giant)); }
                Box::new(content.child(Text::styled("mochiOS Kinako", TextRole::DisplayMedium))
                    .child(self.metadata(format!("Version {}", model.release)))
                    .child(Self::paragraph("Set up mochiOS on your device."))
                    .child(Self::paragraph("Review the legal documents and privacy choices, then choose where to install mochiOS.")))
            }
            Page::Terms => Box::new(self.heading("Terms of Use", "Review the terms before continuing.")
                .child(self.metadata(format!("Document version: {}", docs.catalog.document_version)))
                .child(Self::paragraph(docs.text("terms.txt")))),
            Page::Privacy => Box::new(self.heading("Privacy Policy", "Review the privacy policy. Data sharing is controlled by the choices on the following screens.")
                .child(self.metadata(format!("Document version: {}", docs.catalog.document_version)))
                .child(Self::paragraph(docs.text("privacy.txt")))),
            Page::Licenses => {
                let mut names = List::new();
                for (index, license) in docs.catalog.licenses.iter().enumerate() {
                    let version = license.version.as_deref().unwrap_or(if license.name == "mochiOS" { &model.release } else { "Version not specified" });
                    names = names.row(core.row(&license.name, format!("{version} · {}", license.license), model.license == index, Action::License(index)));
                }
                let license = &docs.catalog.licenses[model.license];
                let detail = Self::stack().child(Text::styled(&license.name, TextRole::TitleMedium))
                    .child(self.metadata(&license.license))
                    .child(Text::label("Copyright")).child(Self::paragraph(docs.text(&license.copyright_file)))
                    .child(Text::label("License text")).child(Self::paragraph(docs.text(&license.body_file)));
                Box::new(self.heading("Open Source Licenses", "Select software to read its copyright notice and license.")
                    .child(NavigationSplitView::new(
                        Scroll::new(core.license_list_scroll.clone()).content(names),
                        Scroll::new(core.license_detail_scrolls[model.license].clone()).content(ContentArea::pane(detail)))
                        .fixed_sidebar(200.0).minimum_detail_width(0.0)
                        .sidebar_insets(appcore::viewkit::edge_insets::EdgeInsets::ZERO)
                        .layout().flex_grow(1.0)))
            }
            Page::Collection(category) => {
                let selected = model.drafts[category.index()];
                let theme = self.theme.get();
                let choice = |label: &str, allow: bool| {
                    let active = selected == Some(allow);
                    core.button(label, Action::Choose(allow))
                        .size(ButtonSize::Large)
                        .content(VStack::new().alignment(StackAlignment::Stretch).distribution(StackDistribution::Center)
                            .child(Text::body(label).alignment(TextAlignment::Center)
                                .color(if active { Color::WHITE } else { theme.colors.text_primary })))
                        .accessibility_label(label)
                        .accessibility_selected(active)
                        .style(if active { ButtonStyle::Primary } else { ButtonStyle::Standard })
                        .layout()
                        .frame(theme.spacing.giant * 3.0, theme.spacing.huge)
                };
                let choices: Box<dyn View> = if compact {
                    Box::new(VStack::new().gap(StackGap::Medium)
                        .child(choice("Agree", true))
                        .child(choice("Disagree", false)))
                } else {
                    Box::new(HStack::new().gap(StackGap::Large)
                        .child(choice("Agree", true))
                        .child(choice("Disagree", false)))
                };
                let mut content = VStack::new()
                    .alignment(StackAlignment::Stretch)
                    .gap(StackGap::ExtraLarge)
                    .child(VStack::new().alignment(StackAlignment::Stretch).gap(StackGap::Medium)
                        .child(Text::styled(model.page.title(), TextRole::TitleLarge).alignment(TextAlignment::Center))
                        .child(Text::body(docs.text(&docs.disclosure(category, "en").summary_file))
                            .alignment(TextAlignment::Center).color(theme.colors.text_secondary)))
                    .child(HStack::new().child(Spacer::new()).child(choices).child(Spacer::new()))
                    .child(HStack::new().child(Spacer::new())
                        .child(core.button(if model.details[category.index()] { "Hide Details" } else { "Show Details" }, Action::Details).toolbar())
                        .child(Spacer::new()));
                if model.details[category.index()] {
                    for field in &docs.disclosure(category, "en").fields {
                        content = content.child(self.info(&field.title, docs.text(&field.file)));
                    }
                }
                Box::new(content)
            }
            Page::ConsentSummary => {
                let mut content = self.heading("Privacy & Consent", "Review your choices before choosing an installation destination.");
                for (title, value, page) in [
                    ("Terms of Use", if model.record.terms.is_some() { "Agreed" } else { "Not agreed" }, Page::Terms),
                    ("Privacy Policy", if model.record.privacy.is_some() { "Reviewed" } else { "Not reviewed" }, Page::Privacy),
                    ("Open Source Licenses", if model.record.open_source_licenses.is_some() { "Reviewed" } else { "Not reviewed" }, Page::Licenses),
                ] { content = content.child(self.summary_row(core, title, value, page)); }
                for category in DataCategory::ALL {
                    content = content.child(self.summary_row(core, Page::Collection(category).title(), model.record.permission(category).map_or("Not selected", |p| if p.agreed() { "Agree" } else { "Disagree" }), Page::Collection(category)));
                }
                Box::new(content)
            }
            Page::Destination => {
                let mut content = self.heading("Choose a Destination", "Select a disk for mochiOS. The running system and installation media cannot be used.")
                    .child(core.button("Refresh Disks", Action::Refresh).toolbar().enabled(model.busy.is_none()));
                if let Some(inventory) = &model.inventory {
                    if inventory.disks.is_empty() { content = content.child(Self::paragraph("No physical disks are available.")); }
                    for disk in &inventory.disks {
                        content = content.child(core.button(&disk.entry.name, Action::SelectDisk(disk.entry.id))
                            .enabled(disk.blocked.is_none()).accessibility_selected(model.selected_disk == Some(disk.entry.id))
                            .style(if model.selected_disk == Some(disk.entry.id) { ButtonStyle::Primary } else { ButtonStyle::Standard }))
                            .child(self.metadata(format!("{} · Disk ID {} · {}-byte sectors", bytes(disk.entry.capacity), disk.entry.id, disk.entry.sector_bytes)));
                        if let Some(reason) = &disk.blocked { content = content.child(self.metadata(reason)); }
                    }
                } else if model.busy == Some(Busy::Discovering) { content = content.child(Self::paragraph("Reading disk information…")); }
                Box::new(content)
            }
            Page::Storage => {
                let mut content = self.heading("Storage Configuration", "Review the destination and storage layout.");
                if let Some(disk) = model.target() { content = content.child(self.info("Destination", format!("{} · {}", disk.entry.name, bytes(disk.entry.capacity)))); }
                if let Some(plan) = &model.plan {
                    for partition in &plan.partitions { content = content.child(Self::paragraph(partition)); }
                } else if model.busy == Some(Busy::Preparing) {
                    content = content.child(Self::paragraph(model.progress.as_ref().map_or("Checking storage configuration…", |progress| progress.stage.as_str())));
                } else {
                    content = content.child(self.metadata("A proposed partition layout has not been provided."));
                    if let Some(inventory) = &model.inventory {
                        for part in inventory.snapshot.entries.iter().filter(|e| e.parent == model.selected_disk) {
                            content = content.child(self.info(if part.kind == appcore::storage::Kind::OutsidePartitions { "Outside partitions" } else { "Existing partition" }, format!("{} · {}", part.name, bytes(part.capacity))));
                        }
                    }
                }
                Box::new(content)
            }
            Page::Review => {
                let mut content = self.heading("Review Installation", "Review every change before starting installation.")
                    .child(self.info("System", format!("mochiOS Kinako {}", model.release)));
                if let Some(disk) = model.target() { content = content.child(self.info("Destination", format!("{} · {} · ID {}", disk.entry.name, bytes(disk.entry.capacity), disk.entry.id))); }
                if let Some(plan) = &model.plan {
                    content = content.child(self.info("Partition layout", plan.partitions.join("\n")))
                        .child(self.info("Volumes to format", plan.formats.join("\n")))
                        .child(self.info("Data to be removed", &plan.erased_data))
                        .child(self.info("Installation steps", plan.steps.join("\n")));
                } else { content = content.child(Self::paragraph(PROVIDER_GAP)); }
                Box::new(content)
            }
            Page::Installing => {
                let mut content = self.heading(if model.error.is_some() { "Installation Failed" } else { "Installing mochiOS" }, "Keep your device powered on until installation finishes.");
                if let Some(progress) = &model.progress {
                    content = content.child(Self::paragraph(&progress.stage));
                    if let (Some(done), Some(total)) = (progress.completed, progress.total) {
                        if total > 0 && done <= total { content = content.child(ProgressBar::new(done as f32 / total as f32).accessibility_label("Installation progress"))
                            .child(self.metadata(format!("{} of {}", bytes(done), bytes(total)))); }
                    }
                }
                if model.error.is_some() && model.busy.is_none() { content = content.child(core.button("Choose a Destination", Action::Recover)); }
                Box::new(content)
            }
            Page::Complete => Box::new(self.heading("Installation Complete", "mochiOS Kinako is ready to use.")
                .child(Self::paragraph("Disconnect any installation media before starting from your mochiOS disk."))
                .child(self.metadata(if core.supports_reboot() { "Restart to begin using mochiOS." } else { "Restart is unavailable in this environment." }))
                .child(core.button("Restart", Action::Reboot).style(ButtonStyle::Primary).enabled(model.installed.is_some() && core.supports_reboot() && model.busy.is_none()))),
        }
    }
    fn summary_row(&self, core: &Controller, title: &str, value: &str, page: Page) -> HStack {
        HStack::new()
            .alignment(StackAlignment::Center)
            .gap(StackGap::Medium)
            .child(self.info(title, value).layout().flex_grow(1.0))
            .child(core.button("Change…", Action::Edit(page)).toolbar())
    }
    fn sidebar(&self) -> VStack {
        let model = self.core.model.borrow();
        let mut sidebar = Self::stack()
            .gap(StackGap::Small)
            .child(Text::label("Install mochiOS"));
        for page in Page::ALL {
            sidebar = sidebar.child(
                Text::metadata(format!("{:02}  {}", page.index() + 1, page.title()))
                    .weight(if page == model.page { 600 } else { 400 })
                    .color(if page == model.page {
                        self.theme.get().colors.accent
                    } else {
                        self.theme.get().colors.text_secondary
                    }),
            );
        }
        sidebar
    }
    fn footer(&self) -> HStack {
        let model = self.core.model.borrow();
        let core = &self.core;
        let mut footer = HStack::new()
            .alignment(StackAlignment::Center)
            .gap(StackGap::Small);
        if model.page != Page::Welcome && !matches!(model.page, Page::Installing | Page::Complete) {
            footer = footer.child(
                core.button(
                    if model.editing {
                        "Cancel Changes"
                    } else {
                        "Back"
                    },
                    Action::Back,
                )
                .enabled(model.busy.is_none()),
            );
        }
        footer = footer.child(Spacer::new());
        match model.page {
            Page::Welcome => footer.child(
                core.button("Get Started", Action::Next)
                    .style(ButtonStyle::Primary)
                    .enabled(model.can_next()),
            ),
            Page::Terms => footer
                .child(
                    core.button("Disagree", Action::DisagreeTerms)
                        .enabled(model.documents.is_ok()),
                )
                .child(
                    core.button("Agree and Continue", Action::AgreeTerms)
                        .style(ButtonStyle::Primary)
                        .enabled(model.documents.is_ok()),
                ),
            Page::Privacy => footer.child(
                core.button("Continue", Action::ConfirmPrivacy)
                    .style(ButtonStyle::Primary)
                    .enabled(model.documents.is_ok()),
            ),
            Page::Licenses => footer.child(
                core.button("Continue", Action::ConfirmLicenses)
                    .style(ButtonStyle::Primary)
                    .enabled(model.documents.is_ok()),
            ),
            Page::Review => footer.child(
                core.button("Install…", Action::RequestInstall)
                    .style(ButtonStyle::Primary)
                    .enabled(model.plan.is_some() && model.consent_ready() && model.busy.is_none()),
            ),
            Page::Installing | Page::Complete => footer,
            _ => footer.child(
                core.button("Continue", Action::Next)
                    .style(ButtonStyle::Primary)
                    .enabled(model.can_next()),
            ),
        }
    }
    fn responsive_footer(&self, compact: bool) -> Box<dyn View> {
        let model = self.core.model.borrow();
        if compact && model.page == Page::Terms {
            let back = self.core.button(
                if model.editing {
                    "Cancel Changes"
                } else {
                    "Back"
                },
                Action::Back,
            );
            Box::new(
                Self::stack()
                    .gap(StackGap::Small)
                    .child(
                        HStack::new()
                            .child(back)
                            .child(Spacer::new())
                            .child(self.core.button("Disagree", Action::DisagreeTerms)),
                    )
                    .child(
                        HStack::new().child(Spacer::new()).child(
                            self.core
                                .button("Agree and Continue", Action::AgreeTerms)
                                .style(ButtonStyle::Primary)
                                .enabled(model.documents.is_ok()),
                        ),
                    ),
            )
        } else {
            drop(model);
            Box::new(self.footer())
        }
    }
    fn root(&self, wide: bool, compact: bool) -> Box<dyn View> {
        let model = self.core.model.borrow();
        let page = model.page;
        let error = model.error.clone();
        drop(model);
        let collection = matches!(page, Page::Collection(_));
        let mut detail = Self::stack().distribution(if collection { StackDistribution::Center } else { StackDistribution::Start }).child(
            self.page(compact)
                .layout()
                .flex_grow(f32::from(page == Page::Licenses)),
        );
        if let Some(error) = error {
            detail =
                detail.child(Self::paragraph(error).color(self.theme.get().colors.destructive));
        }
        let detail: Box<dyn View> = if page == Page::Licenses {
            Box::new(ContentArea::pane(detail))
        } else {
            Box::new(
                Scroll::new(self.core.scrolls[page.index()].clone())
                    .content(if collection {
                        ContentArea::pane(detail).maximum_width(self.theme.get().layout.form_width)
                    } else { ContentArea::pane(detail) }),
            )
        };
        let content: Box<dyn View> = if wide {
            Box::new(
                NavigationSplitView::new(Scroll::vertical(self.sidebar()), detail)
                    .shows_divider(false),
            )
        } else {
            Box::new(detail)
        };
        Box::new(
            Surface::app().content(
                VStack::new()
                    .alignment(StackAlignment::Stretch)
                    .gap(StackGap::None)
                    .child(content.layout().flex_grow(1.0))
                    .child(Toolbar::bottom(self.responsive_footer(compact))),
            ),
        )
    }
    fn ensure_cached(&self, bounds: Rect) {
        let wide = bounds.size.width >= 900.0;
        let revision = self.core.revision.get();
        let compact = bounds.size.width < 620.0;
        let needs = self.cached.borrow().as_ref().is_none_or(|c| {
            c.revision != revision
                || c.wide != wide
                || c.compact != compact
                || c.theme != self.theme.get()
        });
        if needs {
            *self.cached.borrow_mut() = Some(Cached {
                revision,
                wide,
                compact,
                theme: self.theme.get(),
                view: self.root(wide, compact),
            });
        }
    }
    fn modal(&self) -> Option<appcore::viewkit::components::Dialog<VStack>> {
        let model = self.core.model.borrow();
        let modal = model.dialog?;
        let queue = self.core.clone();
        let mut content = Self::stack();
        content = if modal == Modal::Disagree {
            content
                .child(Text::styled("Agreement Required", TextRole::TitleSmall))
                .child(Self::paragraph(
                    "You must agree to the Terms of Use to install mochiOS.",
                ))
                .child(
                    HStack::new()
                        .gap(StackGap::Small)
                        .child(self.core.button("Back to Terms", Action::DismissDialog))
                        .child(
                            self.core
                                .button("Quit Installer", Action::Quit)
                                .style(ButtonStyle::Danger),
                        ),
                )
        } else {
            content
                .child(Text::styled("Confirm Installation", TextRole::TitleSmall))
                .child(Self::paragraph(
                    model
                        .plan
                        .as_ref()
                        .map_or("An installation plan is required.".into(), |p| {
                            format!("Install on {}? {}", p.target.name, p.erased_data)
                        }),
                ))
                .child(
                    HStack::new()
                        .gap(StackGap::Small)
                        .child(self.core.button("Cancel", Action::DismissDialog))
                        .child(
                            self.core
                                .button("Erase and Install", Action::ConfirmInstall)
                                .style(ButtonStyle::Danger),
                        ),
                )
        };
        Some(
            appcore::viewkit::components::Dialog::new()
                .accessibility_label(if modal == Modal::Disagree {
                    "Agreement Required"
                } else {
                    "Confirm Installation"
                })
                .on_dismiss(move || queue.submit(Action::DismissDialog))
                .content(content),
        )
    }
    fn modal_bounds(bounds: Rect) -> Rect {
        let width = (bounds.size.width - 32.0).clamp(0.0, 460.0);
        let height = (bounds.size.height - 32.0).clamp(0.0, 260.0);
        Rect::new(
            bounds.origin.x + (bounds.size.width - width) * 0.5,
            bounds.origin.y + (bounds.size.height - height) * 0.5,
            width,
            height,
        )
    }
}
impl View for InstallerView {
    fn measure(&self, constraints: Constraints, _: &mut MeasureContext<'_>) -> Size {
        constraints.constrain(constraints.maximum)
    }
    fn paint(&self, bounds: Rect, context: &mut PaintContext<'_>) {
        self.theme.set(*context.theme);
        self.core.flush();
        self.ensure_cached(bounds);
        self.cached
            .borrow()
            .as_ref()
            .unwrap()
            .view
            .paint(bounds, context);
        if let Some(dialog) = self.modal() {
            Rectangle::new()
                .color(RectangleColor::Custom(context.theme.shell.scrim))
                .paint(bounds, context);
            dialog.paint(Self::modal_bounds(bounds), context);
        }
    }
    fn handle_event(
        &self,
        bounds: Rect,
        event: &ViewEvent,
        context: &mut EventContext<'_>,
    ) -> EventResult {
        self.theme.set(*context.theme());
        self.ensure_cached(bounds);
        let result = if let Some(dialog) = self.modal() {
            let _ = dialog.handle_event(Self::modal_bounds(bounds), event, context);
            EventResult::Consumed
        } else {
            self.cached
                .borrow()
                .as_ref()
                .unwrap()
                .view
                .handle_event(bounds, event, context)
        };
        if self.core.flush() {
            context.request_redraw();
        }
        result
    }
}
fn bytes(value: u64) -> String {
    format!("{:.1} GiB", value as f64 / 1073741824.0)
}
