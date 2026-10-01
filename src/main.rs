use appcore::prelude::*;

const STEP_WELCOME: usize = 0;
const STEP_DESTINATION: usize = 1;
const STEP_LAYOUT: usize = 2;
const STEP_REVIEW: usize = 3;
const LAST_STEP: usize = STEP_REVIEW;

#[derive(Clone)]
struct InstallationPreview {
    product: &'static str,
    release: &'static str,
    architecture: &'static str,
    firmware: &'static str,
    target_name: &'static str,
    target_identifier: &'static str,
    target_capacity: &'static str,
    system_capacity: &'static str,
    data_capacity: &'static str,
}

impl InstallationPreview {
    const fn example() -> Self {
        Self {
            product: "mochiOS",
            release: "Installer media",
            architecture: "x86_64",
            firmware: "UEFI / GPT",
            target_name: "VirtIO Block Device",
            target_identifier: "Disk 0",
            target_capacity: "100 GB",
            system_capacity: "8.5 GB",
            data_capacity: "About 91.5 GB",
        }
    }
}

struct InstallerApp {
    step: State<usize>,
    selected_disk: State<usize>,
    preview: InstallationPreview,
}

impl InstallerApp {
    fn sidebar(&self) -> impl View + 'static {
        let current = self.step.get();
        VStack::new()
            .alignment(StackAlignment::Stretch)
            .gap(StackGap::Large)
            .child(
                VStack::new()
                    .alignment(StackAlignment::Stretch)
                    .gap(StackGap::ExtraSmall)
                    .child(Text::styled("Install mochiOS", TextRole::TitleMedium))
                    .child(
                        Text::styled("Installation preview", TextRole::Caption)
                            .tone(TextTone::Secondary),
                    ),
            )
            .child(
                VStack::new()
                    .alignment(StackAlignment::Stretch)
                    .gap(StackGap::Small)
                    .child(Self::step_row(1, "Welcome", current, STEP_WELCOME))
                    .child(Self::step_row(2, "Destination", current, STEP_DESTINATION))
                    .child(Self::step_row(3, "Storage", current, STEP_LAYOUT))
                    .child(Self::step_row(4, "Review", current, STEP_REVIEW)),
            )
            .child(Spacer::new())
            .child(
                Text::styled("No changes will be made", TextRole::Caption)
                    .tone(TextTone::Secondary),
            )
    }

    fn step_row(number: usize, label: &str, current: usize, step: usize) -> impl View + 'static {
        let tone = if step < current {
            BadgeTone::Success
        } else if step == current {
            BadgeTone::Accent
        } else {
            BadgeTone::Neutral
        };
        HStack::new()
            .alignment(StackAlignment::Center)
            .gap(StackGap::Medium)
            .child(Badge::new(number.to_string()).tone(tone))
            .child(
                Text::styled(label.to_owned(), TextRole::Body)
                    .weight(if step == current { 600 } else { 400 })
                    .tone(if step > current {
                        TextTone::Secondary
                    } else {
                        TextTone::Primary
                    }),
            )
    }

    fn welcome(&self) -> Box<dyn View + 'static> {
        Box::new(
            VStack::new()
                .alignment(StackAlignment::Center)
                .distribution(StackDistribution::Center)
                .gap(StackGap::ExtraLarge)
                .child(
                    Icon::new(SymbolName::Internaldrive)
                        .size(Theme::current().spacing.giant)
                        .color(Theme::current().colors.accent),
                )
                .child(
                    VStack::new()
                        .alignment(StackAlignment::Center)
                        .gap(StackGap::Small)
                        .child(
                            Text::styled("Install mochiOS", TextRole::DisplayMedium)
                                .alignment(TextAlignment::Center),
                        )
                        .child(
                            Text::styled(
                                "Choose a destination and review the storage layout before installation.",
                                TextRole::Body,
                            )
                            .tone(TextTone::Secondary)
                            .alignment(TextAlignment::Center),
                        ),
                )
                .child(
                    Surface::pane().content(
                        Padding::all(Theme::current().spacing.large).content(
                            HStack::new()
                                .alignment(StackAlignment::Center)
                                .gap(StackGap::Medium)
                                .child(
                                    Icon::new(SymbolName::Info)
                                        .color(Theme::current().shell.secondary_text),
                                )
                                .child(
                                    Text::styled(
                                        "This build is a UI preview. It can inspect installation information, but it cannot modify storage.",
                                        TextRole::Caption,
                                    )
                                    .tone(TextTone::Secondary),
                                ),
                        ),
                    ),
                )
                .child(self.continue_button(STEP_DESTINATION)),
        )
    }

    fn destination(&self) -> Box<dyn View + 'static> {
        let preview = self.preview.clone();
        Box::new(
            VStack::new()
                .alignment(StackAlignment::Stretch)
                .gap(StackGap::ExtraLarge)
                .child(
                    PageHeader::new("Choose a destination")
                        .subtitle("Select the disk that will contain mochiOS and its Data volume."),
                )
                .child(
                    HStack::new()
                        .alignment(StackAlignment::Center)
                        .gap(StackGap::Medium)
                        .child(Badge::new("Preview data").tone(BadgeTone::Warning))
                        .child(
                            Text::styled(
                                "Storage discovery is not connected yet.",
                                TextRole::Caption,
                            )
                            .tone(TextTone::Secondary),
                        ),
                )
                .child(
                    Surface::pane().content(
                        Padding::all(Theme::current().spacing.extra_large).content(
                            HStack::new()
                                .alignment(StackAlignment::Center)
                                .gap(StackGap::Large)
                                .child(RadioButton::new(self.selected_disk.binding(), 0).label(""))
                                .child(
                                    Icon::new(SymbolName::Internaldrive)
                                        .size(Theme::current().layout.prominent_icon_button_size),
                                )
                                .child(
                                    VStack::new()
                                        .alignment(StackAlignment::Stretch)
                                        .gap(StackGap::ExtraSmall)
                                        .child(
                                            Text::styled(preview.target_name, TextRole::Body)
                                                .weight(600),
                                        )
                                        .child(
                                            Text::styled(
                                                format!(
                                                    "{}  ·  {}  ·  {}",
                                                    preview.target_identifier,
                                                    preview.target_capacity,
                                                    preview.firmware
                                                ),
                                                TextRole::Caption,
                                            )
                                            .tone(TextTone::Secondary),
                                        ),
                                )
                                .child(Spacer::new())
                                .child(Badge::new("Available").tone(BadgeTone::Success)),
                        ),
                    ),
                )
                .child(Spacer::new())
                .child(self.navigation_buttons(STEP_WELCOME, STEP_LAYOUT)),
        )
    }

    fn layout_preview(&self) -> Box<dyn View + 'static> {
        let preview = self.preview.clone();
        Box::new(
            VStack::new()
                .alignment(StackAlignment::Stretch)
                .gap(StackGap::ExtraLarge)
                .child(
                    PageHeader::new("Storage layout")
                        .subtitle("Data automatically uses the capacity left after system partitions."),
                )
                .child(
                    VStack::new()
                        .alignment(StackAlignment::Stretch)
                        .gap(StackGap::Small)
                        .child(
                            HStack::new()
                                .alignment(StackAlignment::Center)
                                .child(
                                    Text::styled(preview.target_name, TextRole::Body).weight(600),
                                )
                                .child(Spacer::new())
                                .child(
                                    Text::styled(preview.target_capacity, TextRole::Body)
                                        .tone(TextTone::Secondary),
                                ),
                        )
                        .child(
                            ProgressBar::new(0.085)
                                .accessibility_label("Space reserved for system partitions"),
                        ),
                )
                .child(
                    VStack::new()
                        .alignment(StackAlignment::Stretch)
                        .gap(StackGap::Large)
                        .child(Self::storage_row(
                            SymbolName::Lock,
                            "EFI and System",
                            "Boot files and the read-only operating system",
                            preview.system_capacity,
                        ))
                        .child(Self::storage_row(
                            SymbolName::Folder,
                            "Data",
                            "Applications, accounts, documents, and settings",
                            preview.data_capacity,
                        )),
                )
                .child(
                    Surface::pane().content(
                        Padding::all(Theme::current().spacing.large).content(
                            Text::styled(
                                "The 128 MB development image size is not an installation limit. On a 100 GB disk, Data will use the remaining space.",
                                TextRole::Caption,
                            )
                            .tone(TextTone::Secondary),
                        ),
                    ),
                )
                .child(Spacer::new())
                .child(self.navigation_buttons(STEP_DESTINATION, STEP_REVIEW)),
        )
    }

    fn storage_row(
        symbol: SymbolName,
        title: &str,
        description: &str,
        capacity: &str,
    ) -> impl View + 'static {
        HStack::new()
            .alignment(StackAlignment::Center)
            .gap(StackGap::Large)
            .child(Icon::new(symbol))
            .child(
                VStack::new()
                    .alignment(StackAlignment::Stretch)
                    .gap(StackGap::ExtraSmall)
                    .child(Text::styled(title.to_owned(), TextRole::Body).weight(600))
                    .child(
                        Text::styled(description.to_owned(), TextRole::Caption)
                            .tone(TextTone::Secondary),
                    ),
            )
            .child(Spacer::new())
            .child(Text::styled(capacity.to_owned(), TextRole::Body).tone(TextTone::Secondary))
    }

    fn review(&self) -> Box<dyn View + 'static> {
        let preview = self.preview.clone();
        Box::new(
            VStack::new()
                .alignment(StackAlignment::Stretch)
                .gap(StackGap::ExtraLarge)
                .child(
                    PageHeader::new("Ready to install")
                        .subtitle("Review the information that a real installation will require."),
                )
                .child(
                    Surface::pane().content(
                        Padding::all(Theme::current().spacing.extra_large).content(
                            VStack::new()
                                .alignment(StackAlignment::Stretch)
                                .gap(StackGap::Large)
                                .child(Self::review_row("System", preview.product))
                                .child(Self::review_row("Release", preview.release))
                                .child(Self::review_row("Architecture", preview.architecture))
                                .child(Self::review_row("Destination", preview.target_name))
                                .child(Self::review_row("Capacity", preview.target_capacity))
                                .child(Self::review_row("Data", preview.data_capacity)),
                        ),
                    ),
                )
                .child(
                    HStack::new()
                        .alignment(StackAlignment::Center)
                        .gap(StackGap::Medium)
                        .child(Icon::new(SymbolName::Warning))
                        .child(
                            Text::styled(
                                "A production build must verify the media signature, re-read the GPT, and request secure confirmation before writing.",
                                TextRole::Caption,
                            )
                            .tone(TextTone::Secondary),
                        ),
                )
                .child(Spacer::new())
                .child(
                    HStack::new()
                        .alignment(StackAlignment::Center)
                        .child(self.back_button(STEP_LAYOUT))
                        .child(Spacer::new())
                        .child(Badge::new("UI preview only").tone(BadgeTone::Neutral))
                        .child(
                            Button::new("Install")
                                .style(ButtonStyle::Accent)
                                .size(ButtonSize::Medium)
                                .enabled(false),
                        ),
                ),
        )
    }

    fn review_row(label: &str, value: &str) -> impl View + 'static {
        HStack::new()
            .alignment(StackAlignment::Center)
            .child(Text::styled(label.to_owned(), TextRole::Body).tone(TextTone::Secondary))
            .child(Spacer::new())
            .child(Text::styled(value.to_owned(), TextRole::Body).weight(600))
    }

    fn back_button(&self, destination: usize) -> Button {
        let step = self.step.clone();
        Button::new("Back")
            .style(ButtonStyle::Standard)
            .size(ButtonSize::Medium)
            .on_click(move || step.set(destination))
    }

    fn continue_button(&self, destination: usize) -> Button {
        let step = self.step.clone();
        Button::new("Continue")
            .style(ButtonStyle::Accent)
            .size(ButtonSize::Medium)
            .on_click(move || step.set(destination))
    }

    fn navigation_buttons(&self, back: usize, next: usize) -> impl View + 'static {
        HStack::new()
            .alignment(StackAlignment::Center)
            .child(self.back_button(back))
            .child(Spacer::new())
            .child(self.continue_button(next))
    }
}

impl App for InstallerApp {
    type Body = Box<dyn View + 'static>;

    fn new() -> Self {
        Self {
            step: State::new(STEP_WELCOME),
            selected_disk: State::new(0),
            preview: InstallationPreview::example(),
        }
    }

    fn window(&self) -> WindowOptions {
        WindowOptions::new("Install mochiOS")
            .size(
                Theme::current().layout.standard_window_width,
                Theme::current().layout.standard_window_height,
            )
            .resizable(true)
    }

    fn body(&self, _context: &ViewContext) -> Self::Body {
        let step = self.step.get().min(LAST_STEP);
        let page = match step {
            STEP_WELCOME => self.welcome(),
            STEP_DESTINATION => self.destination(),
            STEP_LAYOUT => self.layout_preview(),
            _ => self.review(),
        };

        Box::new(
            NavigationSplitView::new(self.sidebar(), ContentArea::new(Scroll::vertical(page)))
                .minimum_detail_width(Theme::current().layout.form_width)
                .shows_divider(false),
        )
    }
}

fn main() -> Result<(), ViewKitError> {
    appcore::run::<InstallerApp>()
}
