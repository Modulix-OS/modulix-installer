//! Step 7 — target disk, install mode (whole disk / alongside Windows / free
//! space / manual), swap, and LUKS2+TPM2 encryption. `engine::plan::plan` is
//! the single source of truth for what's about to happen to the disk; this
//! file only collects input and renders `plan`'s output (the "after" bar,
//! the blocked-reason subtitles) — it never re-derives partitioning rules
//! itself.

use crate::a11y::A11ySettings;
use crate::backend::Backends;
use crate::backend::disk::layout::{self, FreeGap};
use crate::backend::disk::ntfs::{NtfsBlocker, NtfsProbe};
use crate::backend::disk::{DiskInfo, PartitionInfo, short_device_name};
use crate::bridge;
use crate::config::{InstallConfig, PartitionMode, PartitioningConfig, SwapMode};
use crate::engine::plan::{self, PlanError, PlanInput, PlanWarning, PreviewRole};
use crate::engine::sizing::{compute_swap_bytes, detect_ram_bytes};
use crate::i18n::tr;
use crate::mx;
use crate::steps::{Step, StepId, ValidityTracker};
use crate::widgets::disk_bar::{DiskBarSegment, SegmentRole, human_bytes};
use crate::widgets::{DiskBar, PartitionEditor, PasswordConfirmEntry, size_dropdown_to_widest};
use adw::prelude::*;
use std::cell::{Cell, RefCell};
use std::path::Path;
use std::rc::Rc;

const MODES: [PartitionMode; 4] = [
    PartitionMode::EntireDisk,
    PartitionMode::AlongsideWindows,
    PartitionMode::FreeSpace,
    PartitionMode::Manual,
];
const SWAP_MODES: [SwapMode; 3] = [SwapMode::None, SwapMode::Standard, SwapMode::Hibernation];
/// Largest-to-smallest, the order `build_clamp_swap` walks downward from the
/// current selection when it doesn't fit.
const SWAP_DOWNWARD: [SwapMode; 3] = [SwapMode::Hibernation, SwapMode::Standard, SwapMode::None];

fn mode_title(mode: PartitionMode) -> String {
    match mode {
        PartitionMode::EntireDisk => tr("Erase entire disk"),
        PartitionMode::AlongsideWindows => tr("Install alongside Windows"),
        PartitionMode::FreeSpace => tr("Use free space"),
        PartitionMode::Manual => tr("Manual"),
    }
}

fn swap_label(mode: SwapMode) -> String {
    match mode {
        SwapMode::None => tr("No swap"),
        SwapMode::Standard => tr("Standard swap"),
        SwapMode::Hibernation => tr("Swap with hibernation support"),
    }
}

/// Small info icon: tooltip on hover, and on keyboard focus (`set_focusable`
/// — GTK only shows a non-focusable widget's tooltip on mouse hover). Not a
/// button: nothing to click, so no misleading affordance.
fn help_icon(text: &str) -> gtk::Image {
    let icon = gtk::Image::from_icon_name("help-about-symbolic");
    icon.add_css_class("dim-label");
    icon.set_focusable(true);
    set_help_text(&icon, text);
    icon
}

fn set_help_text(icon: &gtk::Image, text: &str) {
    icon.set_tooltip_text(Some(text));
    icon.update_property(&[gtk::accessible::Property::Label(text)]);
}

fn swap_string_list() -> gtk::StringList {
    let names: Vec<String> = SWAP_MODES.iter().map(|m| swap_label(*m)).collect();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    gtk::StringList::new(&refs)
}

/// "Windows: X — Modulix: Y (root R, swap S, boot B)" under the
/// alongside-Windows slider. `ntfs_size` is the NTFS partition's *current*
/// size — `Y` is the total space freed up for Modulix (ESP + root + swap),
/// matching what the "after" bar shows to the right of Windows; the
/// parenthesized breakdown spells out what that total is made of so it isn't
/// misread as all going to the root. `esp_bytes`/`swap_bytes` of `0` (BIOS,
/// no swap) drop their own clause instead of showing a misleading "0 MiB".
fn set_windows_label(
    label: &gtk::Label,
    ntfs_size: u64,
    shrink_to: u64,
    esp_bytes: u64,
    swap_bytes: u64,
) {
    let freed = ntfs_size.saturating_sub(shrink_to);
    let root_bytes = freed.saturating_sub(esp_bytes).saturating_sub(swap_bytes);
    let mut breakdown = vec![format!("{} {}", tr("root"), human_bytes(root_bytes))];
    if swap_bytes > 0 {
        breakdown.push(format!("{} {}", tr("swap"), human_bytes(swap_bytes)));
    }
    if esp_bytes > 0 {
        breakdown.push(format!("{} {}", tr("boot"), human_bytes(esp_bytes)));
    }
    label.set_label(&format!(
        "{}: {} — {}: {} ({})",
        tr("Windows"),
        human_bytes(shrink_to),
        tr("Modulix"),
        human_bytes(freed),
        breakdown.join(", "),
    ));
}

fn detect_uefi() -> bool {
    Path::new("/sys/firmware/efi").exists()
}

fn detect_tpm2() -> bool {
    Path::new("/sys/class/tpm/tpm0").exists()
}

#[derive(Clone)]
struct DiskState {
    disk: DiskInfo,
    partitions: Vec<PartitionInfo>,
    gaps: Vec<FreeGap>,
    ntfs: Option<(String, NtfsProbe)>,
}

fn preview_role_to_bar_role(role: PreviewRole, fs_type: &Option<String>) -> SegmentRole {
    match role {
        PreviewRole::Esp => SegmentRole::Esp,
        PreviewRole::Root => SegmentRole::Root,
        PreviewRole::Swap => SegmentRole::Swap,
        PreviewRole::Home => SegmentRole::Home,
        PreviewRole::Free => SegmentRole::Free,
        PreviewRole::Keep => {
            if fs_type.as_deref() == Some("ntfs") {
                SegmentRole::Windows
            } else {
                SegmentRole::Other
            }
        }
    }
}

fn before_segments(state: &DiskState) -> Vec<DiskBarSegment> {
    let mut items: Vec<(u64, DiskBarSegment)> = state
        .partitions
        .iter()
        .map(|p| {
            let role = if p.is_esp {
                SegmentRole::Esp
            } else if p.fs_type.as_deref() == Some("ntfs") {
                SegmentRole::Windows
            } else {
                SegmentRole::Other
            };
            (
                p.start_bytes,
                DiskBarSegment {
                    label: p
                        .label
                        .clone()
                        .unwrap_or_else(|| short_device_name(&p.path).to_string()),
                    size_bytes: p.size_bytes,
                    role,
                    encrypted: false,
                    used_bytes: p.used_bytes,
                },
            )
        })
        .collect();
    for gap in &state.gaps {
        items.push((
            gap.start_bytes,
            DiskBarSegment {
                label: tr("Free space"),
                size_bytes: gap.size_bytes,
                role: SegmentRole::Free,
                encrypted: false,
                used_bytes: None,
            },
        ));
    }
    items.sort_by_key(|(start, _)| *start);
    items.into_iter().map(|(_, seg)| seg).collect()
}

/// Best-effort config for probing whether `mode` is even reachable given the
/// currently loaded disk — used only to decide row sensitivity/subtitle in
/// the mode picker, never to actually commit anything.
fn feasibility_cfg(mode: PartitionMode, state: &DiskState) -> PartitioningConfig {
    PartitioningConfig {
        mode,
        target_disk: Some(state.disk.path.clone()),
        shrink_to_bytes: state.ntfs.as_ref().map(|(_, probe)| probe.min_size_bytes),
        selected_free_space_id: state
            .gaps
            .iter()
            .max_by_key(|g| g.size_bytes)
            .map(|g| g.id.clone()),
        swap_mode: SwapMode::Standard,
        ..Default::default()
    }
}

fn mode_feasibility(
    mode: PartitionMode,
    state: &DiskState,
    ram_bytes: u64,
    uefi: bool,
) -> Result<(), PlanError> {
    if mode == PartitionMode::Manual {
        // User-driven; the real gate is `revalidate()` against the actual
        // entries, not this picker.
        return Ok(());
    }
    let input = PlanInput {
        disk: state.disk.clone(),
        partitions: state.partitions.clone(),
        gaps: state.gaps.clone(),
        ntfs: state.ntfs.clone(),
        cfg: feasibility_cfg(mode, state),
        ram_bytes,
        uefi,
    };
    plan::plan(&input).map(|_| ())
}

/// What blocker `e` implies for the `AlongsideWindows` row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AlongsideRowState {
    /// Row greyed out. `Some(blocker)` also raises the orange warning strip
    /// (the message moves from the subtitle into the strip).
    Disabled(Option<NtfsBlocker>),
    /// Row stays clickable: clicking it explains and reverts to the previous
    /// mode (the only way `ShrinkTooSmall` — "not enough room to free up" —
    /// is ever reachable).
    Clickable,
}

fn alongside_row_state(e: &PlanError) -> AlongsideRowState {
    match e {
        PlanError::NoWindows => AlongsideRowState::Disabled(None),
        PlanError::NtfsBlocked(b) => AlongsideRowState::Disabled(Some(*b)),
        _ => AlongsideRowState::Clickable,
    }
}

/// Unmounts every partition on `disk_path` (GParted refuses a mounted disk)
/// and launches GParted on it. Runs entirely on the tokio side — see
/// `bridge::spawn`, never call this synchronously from a GTK callback.
/// Best-effort throughout: a stray unmount failure or a GParted crash isn't
/// fatal, the caller always re-reads the disk afterwards regardless.
async fn run_gparted(
    disk_backend: std::sync::Arc<dyn crate::backend::disk::DiskBackend>,
    disk_path: String,
) {
    let node = match disk_backend.device_node(&disk_path).await {
        Ok(node) => node,
        Err(e) => {
            eprintln!("gparted: couldn't resolve device node for {disk_path}: {e}");
            return;
        }
    };
    if let Ok(partitions) = disk_backend.list_partitions(&disk_path).await {
        for p in partitions {
            let _ = disk_backend.unmount(&p.path).await;
        }
    }
    // Arguments passed as an array, never through a shell; `node` came out
    // of `device_node`, itself resolved from a `list_disks`-enumerated path
    // — never built by concatenating user input.
    if let Err(e) = tokio::process::Command::new("gparted")
        .arg(&node)
        .status()
        .await
    {
        eprintln!("gparted: failed to launch on {node}: {e}");
    }
}

/// Bundles the callbacks/shared state `build_load_disk`'s closure needs to
/// invoke once a disk finishes loading — kept as one struct instead of five
/// separate parameters (clippy's `too_many_arguments`).
struct LoadDiskCallbacks {
    revalidate: Rc<dyn Fn()>,
    refresh_windows_scale: Rc<dyn Fn()>,
    clamp_swap: Rc<dyn Fn()>,
    alongside_infeasible: Rc<RefCell<Option<PlanError>>>,
}

pub struct PartitioningStep {
    widget: gtk::Widget,
    disk_label: gtk::Label,
    disk_dropdown: gtk::DropDown,
    refresh_button: gtk::Button,
    toggle_before: adw::Toggle,
    toggle_after: adw::Toggle,
    ntfs_warning_group: adw::PreferencesGroup,
    ntfs_warning_label: gtk::Label,
    mode_group: adw::PreferencesGroup,
    mode_rows: Vec<adw::ActionRow>,
    mode_checks: Vec<gtk::CheckButton>,
    detail_group: adw::PreferencesGroup,
    detail_stack: gtk::Stack,
    windows_scale: gtk::Scale,
    windows_label: gtk::Label,
    windows_banner: adw::Banner,
    freespace_row: adw::ActionRow,
    freespace_dropdown: gtk::DropDown,
    manual_editor: PartitionEditor,
    manual_banner: adw::Banner,
    gparted_button: gtk::Button,
    gparted_label: gtk::Label,
    swap_group: adw::PreferencesGroup,
    swap_row: adw::ActionRow,
    swap_dropdown: gtk::DropDown,
    swap_banner: adw::Banner,
    swap_help: gtk::Image,
    encryption_group: adw::PreferencesGroup,
    encryption_row: adw::ExpanderRow,
    encryption_help: gtk::Image,
    password_widget: PasswordConfirmEntry,
    tpm2_row: adw::SwitchRow,
    tpm2_help: gtk::Image,
    tpm2_pin_row: adw::PasswordEntryRow,
    warning_banner: adw::Banner,
    before_bar: DiskBar,
    after_bar: DiskBar,

    disks: Rc<RefCell<Vec<DiskInfo>>>,
    disk_state: Rc<RefCell<Option<DiskState>>>,
    selected_mode: Rc<Cell<PartitionMode>>,
    selected_swap: Rc<Cell<SwapMode>>,
    encryption_enabled: Rc<Cell<bool>>,
    tpm2_enabled: Rc<Cell<bool>>,
    shrink_to_bytes: Rc<Cell<Option<u64>>>,
    /// NTFS partition's current size, cached alongside `shrink_to_bytes` so
    /// `set_windows_label`'s Y value doesn't need to re-borrow `disk_state`.
    windows_ntfs_size: Rc<Cell<u64>>,
    /// Current `AlongsideWindows` blocker, if it's an `NtfsBlocked` one —
    /// remembered so `retranslate()` can re-apply `ntfs_warning_label`.
    ntfs_blocker: Rc<Cell<Option<NtfsBlocker>>>,
    selected_gap_id: Rc<RefCell<Option<String>>>,
    ram_bytes: Rc<Cell<u64>>,
    uefi: bool,
    is_fake: bool,
    validity: ValidityTracker,
}

impl PartitioningStep {
    pub fn new(backends: &Backends, runtime: &tokio::runtime::Handle, a11y: &A11ySettings) -> Self {
        let uefi = detect_uefi();

        let disk_label = gtk::Label::builder()
            .label(tr("Target disk"))
            .xalign(0.0)
            .build();
        let disk_dropdown = gtk::DropDown::from_strings(&[]);
        disk_dropdown.set_hexpand(true);
        let refresh_button = gtk::Button::from_icon_name("view-refresh-symbolic");
        refresh_button.set_tooltip_text(Some(&tr("Refresh")));
        refresh_button.add_css_class("flat");
        let disk_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        disk_box.append(&disk_label);
        disk_box.append(&disk_dropdown);
        disk_box.append(&refresh_button);

        let toggle_before = adw::Toggle::builder()
            .name("before")
            .label(tr("Current layout"))
            .build();
        let toggle_after = adw::Toggle::builder()
            .name("after")
            .label(tr("After installation"))
            .build();
        let view_toggle_group = adw::ToggleGroup::new();
        view_toggle_group.add(toggle_before.clone());
        view_toggle_group.add(toggle_after.clone());
        view_toggle_group.set_active(1);

        let before_bar = DiskBar::new();
        let after_bar = DiskBar::new();
        let bar_stack = gtk::Stack::builder()
            .transition_type(gtk::StackTransitionType::Crossfade)
            .build();
        bar_stack.add_named(&before_bar.widget(), Some("before"));
        bar_stack.add_named(&after_bar.widget(), Some("after"));
        bar_stack.set_visible_child_name("after");
        {
            let bar_stack = bar_stack.clone();
            view_toggle_group.connect_active_notify(move |tg| {
                bar_stack.set_visible_child_name(if tg.active() == 0 { "before" } else { "after" });
            });
        }

        let warning_banner = adw::Banner::new("");

        let pinned_box = gtk::Box::new(gtk::Orientation::Vertical, 12);
        pinned_box.set_margin_top(12);
        pinned_box.set_margin_bottom(12);
        pinned_box.set_margin_start(12);
        pinned_box.set_margin_end(12);
        pinned_box.append(&disk_box);
        pinned_box.append(&view_toggle_group);
        pinned_box.append(&bar_stack);
        pinned_box.append(&warning_banner);
        let pinned_clamp = adw::Clamp::builder()
            .maximum_size(600)
            .tightening_threshold(400)
            .child(&pinned_box)
            .build();

        let ntfs_warning_icon = gtk::Image::from_icon_name("dialog-warning-symbolic");
        ntfs_warning_icon.set_pixel_size(16);
        let ntfs_warning_label = gtk::Label::builder()
            .wrap(true)
            .xalign(0.0)
            .hexpand(true)
            .build();
        let ntfs_warning_box = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        ntfs_warning_box.append(&ntfs_warning_icon);
        ntfs_warning_box.append(&ntfs_warning_label);
        ntfs_warning_box.add_css_class("mx-warning-strip");
        let ntfs_warning_group = adw::PreferencesGroup::new();
        ntfs_warning_group.add(&ntfs_warning_box);
        ntfs_warning_group.set_visible(false);

        let mode_group = adw::PreferencesGroup::builder()
            .title(tr("Installation type"))
            .build();
        let mut mode_rows = Vec::new();
        let mut mode_checks = Vec::new();
        let mut first_check: Option<gtk::CheckButton> = None;
        for mode in MODES {
            let check = gtk::CheckButton::new();
            if let Some(first) = &first_check {
                check.set_group(Some(first));
            } else {
                first_check = Some(check.clone());
            }
            let row = adw::ActionRow::builder().title(mode_title(mode)).build();
            row.add_prefix(&check);
            row.set_activatable_widget(Some(&check));
            mode_group.add(&row);
            mode_rows.push(row);
            mode_checks.push(check);
        }
        mode_checks[0].set_active(true);

        let windows_scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 1.0, 1.0);
        windows_scale.set_hexpand(true);
        // `wrap`+`hexpand` so the label's width tracks its parent instead of
        // its own text — otherwise every slider drag re-measures a
        // different-length string and the whole box jitters horizontally.
        let windows_label = gtk::Label::builder()
            .wrap(true)
            .xalign(0.0)
            .hexpand(true)
            .build();
        let windows_banner =
            adw::Banner::new(&tr("This Windows partition needs a preflight check"));
        let windows_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
        windows_box.append(&windows_banner);
        windows_box.append(&windows_scale);
        windows_box.append(&windows_label);

        let freespace_dropdown = gtk::DropDown::from_strings(&[]);
        let freespace_row = adw::ActionRow::builder()
            .title(tr("Free space region"))
            .build();
        freespace_row.add_suffix(&freespace_dropdown);

        let manual_editor = PartitionEditor::new();
        let manual_banner = adw::Banner::new(&tr(
            "Assign existing partitions or create new ones in free space below. To resize or delete an existing partition, use GParted.",
        ));
        manual_banner.set_revealed(true);
        let gparted_icon = gtk::Image::from_icon_name("drive-harddisk-symbolic");
        let gparted_label = gtk::Label::new(Some(&tr("Partition with GParted…")));
        let gparted_content = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        gparted_content.append(&gparted_icon);
        gparted_content.append(&gparted_label);
        let gparted_button = gtk::Button::builder()
            .child(&gparted_content)
            .halign(gtk::Align::Center)
            .build();
        if backends.is_fake {
            gparted_button.set_sensitive(false);
            gparted_button.set_tooltip_text(Some(&tr("Unavailable with simulated disks")));
        }
        let manual_box = gtk::Box::new(gtk::Orientation::Vertical, 12);
        manual_box.append(&manual_banner);
        manual_box.append(&manual_editor.widget());
        manual_box.append(&gparted_button);

        let empty_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let detail_stack = gtk::Stack::new();
        // `GtkStack` defaults to homogeneous sizing — it reserves height for
        // its *tallest* child (the manual editor's row list) even while
        // showing a much shorter page (e.g. the Windows shrink slider),
        // which left a large blank gap below whichever short page was
        // visible. Size to the current page instead.
        detail_stack.set_vhomogeneous(false);
        detail_stack.set_hhomogeneous(false);
        detail_stack.add_named(&empty_box, Some("none"));
        detail_stack.add_named(&windows_box, Some("windows"));
        detail_stack.add_named(&freespace_row, Some("freespace"));
        detail_stack.add_named(&manual_box, Some("manual"));
        let detail_group = adw::PreferencesGroup::builder()
            .title(tr("Options"))
            .build();
        detail_group.add(&detail_stack);
        detail_group.set_visible(false);

        let swap_dropdown = gtk::DropDown::from_strings(&[]);
        let swap_model = swap_string_list();
        size_dropdown_to_widest(&swap_dropdown, &swap_model);
        swap_dropdown.set_model(Some(&swap_model));
        swap_dropdown.set_selected(1);
        let swap_row = adw::ActionRow::builder().title(tr("Swap")).build();
        let swap_help = help_icon(&tr(
            "Disk space used as extra memory when RAM is full. Hibernation also needs it to save the session to disk.",
        ));
        swap_row.add_prefix(&swap_help);
        swap_row.add_suffix(&swap_dropdown);
        let swap_banner = adw::Banner::new(&tr(
            "Hibernation swap doesn't fit — using standard swap instead",
        ));
        let swap_group = adw::PreferencesGroup::builder().title(tr("Swap")).build();
        swap_group.add(&swap_row);
        swap_group.add(&swap_banner);

        let encryption_row = adw::ExpanderRow::builder()
            .title(tr("Encrypt disk"))
            .subtitle(tr("LUKS2, unlocked via TPM2 when available"))
            .show_enable_switch(true)
            .enable_expansion(false)
            .build();
        let encryption_help = help_icon(&tr(
            "Encrypts the whole disk with LUKS2. A passphrase is required at every boot unless TPM2 unlocking is enabled.",
        ));
        encryption_row.add_prefix(&encryption_help);
        let password_widget = PasswordConfirmEntry::new();
        password_widget.attach_to_expander(&encryption_row);
        let tpm2_row = adw::SwitchRow::builder()
            .title(tr("Use TPM2"))
            .subtitle(tr("Unlock automatically without a passphrase"))
            .build();
        let tpm2_help = help_icon(&tr(
            "The TPM2 chip stores the key and unlocks the disk automatically at boot. Add a PIN to require a code as well.",
        ));
        tpm2_row.add_prefix(&tpm2_help);
        tpm2_row.set_sensitive(false);
        if !detect_tpm2() {
            tpm2_row.set_subtitle(&tr("No TPM2 chip detected"));
        }
        let tpm2_pin_row = adw::PasswordEntryRow::builder()
            .title(tr("TPM2 PIN (optional)"))
            .build();
        tpm2_pin_row.set_visible(false);
        encryption_row.add_row(&tpm2_row);
        encryption_row.add_row(&tpm2_pin_row);
        let encryption_group = adw::PreferencesGroup::builder()
            .title(tr("Encryption"))
            .build();
        encryption_group.add(&encryption_row);

        let content_page = adw::PreferencesPage::new();
        content_page.set_vexpand(true);
        content_page.add(&ntfs_warning_group);
        content_page.add(&mode_group);
        content_page.add(&detail_group);
        content_page.add(&swap_group);
        content_page.add(&encryption_group);

        let container = gtk::Box::new(gtk::Orientation::Vertical, 0);
        container.append(&pinned_clamp);
        container.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        container.append(&content_page);

        {
            let a11y = a11y.clone();
            let before_bar = before_bar.clone();
            let after_bar = after_bar.clone();
            before_bar.set_high_contrast(a11y.high_contrast());
            after_bar.set_high_contrast(a11y.high_contrast());
            a11y.connect_high_contrast_notify(move |settings| {
                before_bar.set_high_contrast(settings.high_contrast());
                after_bar.set_high_contrast(settings.high_contrast());
            });
        }

        let step = Self {
            widget: container.upcast(),
            disk_label,
            disk_dropdown: disk_dropdown.clone(),
            refresh_button: refresh_button.clone(),
            toggle_before,
            toggle_after,
            ntfs_warning_group,
            ntfs_warning_label,
            mode_group,
            mode_rows,
            mode_checks: mode_checks.clone(),
            detail_group,
            detail_stack,
            windows_scale: windows_scale.clone(),
            windows_label,
            windows_banner,
            freespace_row,
            freespace_dropdown: freespace_dropdown.clone(),
            manual_editor,
            manual_banner,
            gparted_button: gparted_button.clone(),
            gparted_label,
            swap_group,
            swap_row,
            swap_dropdown: swap_dropdown.clone(),
            swap_banner: swap_banner.clone(),
            swap_help: swap_help.clone(),
            encryption_group,
            encryption_row: encryption_row.clone(),
            encryption_help: encryption_help.clone(),
            password_widget: password_widget.clone(),
            tpm2_row: tpm2_row.clone(),
            tpm2_help: tpm2_help.clone(),
            tpm2_pin_row: tpm2_pin_row.clone(),
            warning_banner,
            before_bar,
            after_bar,
            disks: Rc::new(RefCell::new(Vec::new())),
            disk_state: Rc::new(RefCell::new(None)),
            selected_mode: Rc::new(Cell::new(PartitionMode::EntireDisk)),
            selected_swap: Rc::new(Cell::new(SwapMode::Standard)),
            encryption_enabled: Rc::new(Cell::new(false)),
            tpm2_enabled: Rc::new(Cell::new(false)),
            shrink_to_bytes: Rc::new(Cell::new(None)),
            windows_ntfs_size: Rc::new(Cell::new(0)),
            ntfs_blocker: Rc::new(Cell::new(None)),
            selected_gap_id: Rc::new(RefCell::new(None)),
            ram_bytes: Rc::new(Cell::new(8u64 * 1024 * 1024 * 1024)),
            uefi,
            is_fake: backends.is_fake,
            validity: ValidityTracker::blocked(tr("Select a target disk")),
        };

        step.wire(backends, runtime);
        step
    }

    fn snapshot_cfg(&self) -> PartitioningConfig {
        PartitioningConfig {
            mode: self.selected_mode.get(),
            target_disk: self
                .disk_state
                .borrow()
                .as_ref()
                .map(|s| s.disk.path.clone()),
            shrink_to_bytes: self.shrink_to_bytes.get(),
            selected_free_space_id: self.selected_gap_id.borrow().clone(),
            swap_mode: self.selected_swap.get(),
            encryption_enabled: self.encryption_enabled.get(),
            encryption_passphrase: self.password_widget.password(),
            tpm2_enabled: self.tpm2_enabled.get(),
            tpm2_pin: self
                .tpm2_enabled
                .get()
                .then(|| self.tpm2_pin_row.text().to_string())
                .filter(|s| !s.is_empty()),
            manual: self.manual_editor.entries(),
        }
    }

    fn wire(&self, backends: &Backends, runtime: &tokio::runtime::Handle) {
        let revalidate: Rc<dyn Fn()> = self.build_revalidate();
        let refresh_windows_scale: Rc<dyn Fn()> = self.build_refresh_windows_scale();
        let clamp_swap: Rc<dyn Fn()> = self.build_clamp_swap();
        // Non-`NoWindows` infeasibility reason for `AlongsideWindows` on the
        // currently loaded disk — `None` means feasible (or truly no
        // Windows, which stays hard-blocked via row sensitivity instead).
        let alongside_infeasible: Rc<RefCell<Option<PlanError>>> = Rc::new(RefCell::new(None));
        // Shared across every mode `CheckButton` — guards the programmatic
        // `set_active` used to revert the radio group, so reverting doesn't
        // re-enter these same handlers.
        let mode_change_guard: Rc<Cell<bool>> = Rc::new(Cell::new(false));

        {
            let ram_bytes = self.ram_bytes.clone();
            let revalidate = revalidate.clone();
            let refresh_windows_scale = refresh_windows_scale.clone();
            bridge::spawn(
                runtime,
                async move { detect_ram_bytes().await },
                move |result| {
                    if let Ok(bytes) = result {
                        ram_bytes.set(bytes);
                    }
                    refresh_windows_scale();
                    revalidate();
                },
            );
        }

        let load_disk = self.build_load_disk(
            backends,
            runtime,
            LoadDiskCallbacks {
                revalidate: revalidate.clone(),
                refresh_windows_scale: refresh_windows_scale.clone(),
                clamp_swap: clamp_swap.clone(),
                alongside_infeasible: alongside_infeasible.clone(),
            },
        );

        {
            let disk_backend = backends.disk.clone();
            let disks = self.disks.clone();
            let disk_dropdown = self.disk_dropdown.clone();
            let load_disk = load_disk.clone();
            bridge::spawn(
                runtime,
                async move { disk_backend.list_disks().await },
                move |result| {
                    let loaded = result.unwrap_or_default();
                    if loaded.is_empty() {
                        return;
                    }
                    let names: Vec<String> = loaded
                        .iter()
                        .map(|d| {
                            format!(
                                "{} ({}{})",
                                d.model,
                                human_bytes(d.size_bytes),
                                if d.is_removable {
                                    format!(", {}", tr("removable"))
                                } else {
                                    String::new()
                                }
                            )
                        })
                        .collect();
                    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
                    let model = gtk::StringList::new(&refs);
                    size_dropdown_to_widest(&disk_dropdown, &model);
                    disk_dropdown.set_model(Some(&model));
                    let first_path = loaded[0].path.clone();
                    *disks.borrow_mut() = loaded;
                    load_disk(first_path);
                },
            );
        }

        {
            let disks = self.disks.clone();
            let load_disk = load_disk.clone();
            self.disk_dropdown.connect_selected_notify(move |dd| {
                let idx = dd.selected() as usize;
                if let Some(disk) = disks.borrow().get(idx) {
                    load_disk(disk.path.clone());
                }
            });
        }

        let refresh_disks = self.build_refresh_disks(backends, runtime, load_disk.clone());

        {
            let refresh_disks = refresh_disks.clone();
            self.refresh_button
                .connect_clicked(move |_| refresh_disks());
        }

        {
            let disk_backend = backends.disk.clone();
            let runtime = runtime.clone();
            let disk_state = self.disk_state.clone();
            let container = self.widget.clone();
            let gparted_button = self.gparted_button.clone();
            let refresh_disks = refresh_disks.clone();
            self.gparted_button.connect_clicked(move |button| {
                let Some(state) = disk_state.borrow().clone() else {
                    return;
                };
                container.set_sensitive(false);
                button.set_sensitive(false);
                let disk_backend = disk_backend.clone();
                let container = container.clone();
                let gparted_button = gparted_button.clone();
                let refresh_disks = refresh_disks.clone();
                bridge::spawn(
                    &runtime,
                    run_gparted(disk_backend, state.disk.path.clone()),
                    move |()| {
                        container.set_sensitive(true);
                        gparted_button.set_sensitive(true);
                        refresh_disks();
                    },
                );
            });
        }

        for (i, check) in self.mode_checks.iter().enumerate() {
            let selected_mode = self.selected_mode.clone();
            let detail_stack = self.detail_stack.clone();
            let detail_group = self.detail_group.clone();
            let swap_group = self.swap_group.clone();
            let revalidate = revalidate.clone();
            let clamp_swap = clamp_swap.clone();
            let mode_checks = self.mode_checks.clone();
            let alongside_infeasible = alongside_infeasible.clone();
            let mode_change_guard = mode_change_guard.clone();
            check.connect_toggled(move |cb| {
                if mode_change_guard.get() {
                    return;
                }
                if !cb.is_active() {
                    return;
                }
                let mode = MODES[i];
                let previous_mode = selected_mode.get();

                let revert = |mode_checks: &[gtk::CheckButton]| {
                    mode_change_guard.set(true);
                    if let Some(prev_idx) = MODES.iter().position(|m| *m == previous_mode) {
                        mode_checks[prev_idx].set_active(true);
                    }
                    mode_change_guard.set(false);
                };

                if mode == PartitionMode::AlongsideWindows && previous_mode != mode {
                    if let Some(err) = alongside_infeasible.borrow().clone() {
                        // Only set for `alongside_row_state(&e) ==
                        // Clickable` blockers (e.g. not enough room on
                        // Windows) — `NtfsBlocked`/`NoWindows` grey the row
                        // out instead, so this is never reached for those.
                        // The row stays clickable here so this is reachable
                        // at all; clicking it just explains why and
                        // reverts, instead of silently doing nothing.
                        revert(&mode_checks);
                        let dialog = adw::AlertDialog::builder()
                            .heading(tr("Can't install alongside Windows"))
                            .body(err.msgid())
                            .build();
                        dialog.add_response("close", &tr("Close"));
                        dialog.set_default_response(Some("close"));
                        dialog.set_close_response("close");
                        let anchor = cb.clone();
                        glib::spawn_future_local(async move {
                            dialog.choose_future(Some(&anchor)).await;
                        });
                        return;
                    }
                }

                selected_mode.set(mode);
                let page_name = match mode {
                    PartitionMode::EntireDisk => "none",
                    PartitionMode::AlongsideWindows => "windows",
                    PartitionMode::FreeSpace => "freespace",
                    PartitionMode::Manual => "manual",
                };
                detail_stack.set_visible_child_name(page_name);
                detail_group.set_visible(mode != PartitionMode::EntireDisk);
                // Manual mode assigns swap per-partition in the editor
                // itself (`ManualMountPoint::Swap`) — the global swap-mode
                // picker only drives auto-sizing for the other three modes.
                swap_group.set_visible(mode != PartitionMode::Manual);
                clamp_swap();
                revalidate();
            });
        }

        {
            let shrink_to_bytes = self.shrink_to_bytes.clone();
            let windows_label = self.windows_label.clone();
            let windows_ntfs_size = self.windows_ntfs_size.clone();
            let selected_swap = self.selected_swap.clone();
            let ram_bytes = self.ram_bytes.clone();
            let uefi = self.uefi;
            let revalidate = revalidate.clone();
            self.windows_scale.connect_value_changed(move |scale| {
                let value = scale.value() as u64;
                shrink_to_bytes.set(Some(value));
                let esp_bytes = if uefi { plan::NEW_ESP_BYTES } else { 0 };
                let swap_bytes =
                    layout::align_up(compute_swap_bytes(selected_swap.get(), ram_bytes.get()));
                set_windows_label(
                    &windows_label,
                    windows_ntfs_size.get(),
                    value,
                    esp_bytes,
                    swap_bytes,
                );
                revalidate();
            });
        }

        {
            let selected_gap_id = self.selected_gap_id.clone();
            let disk_state = self.disk_state.clone();
            let revalidate = revalidate.clone();
            self.freespace_dropdown.connect_selected_notify(move |dd| {
                let idx = dd.selected() as usize;
                let state = disk_state.borrow();
                if let Some(gap) = state.as_ref().and_then(|s| s.gaps.get(idx)) {
                    *selected_gap_id.borrow_mut() = Some(gap.id.clone());
                }
                revalidate();
            });
        }

        {
            let revalidate = revalidate.clone();
            self.manual_editor.connect_changed(move || revalidate());
        }

        {
            let selected_swap = self.selected_swap.clone();
            let revalidate = revalidate.clone();
            let refresh_windows_scale = refresh_windows_scale.clone();
            let clamp_swap = clamp_swap.clone();
            self.swap_dropdown.connect_selected_notify(move |dd| {
                let idx = dd.selected() as usize;
                selected_swap.set(SWAP_MODES.get(idx).copied().unwrap_or(SwapMode::None));
                clamp_swap();
                refresh_windows_scale();
                revalidate();
            });
        }

        {
            let encryption_enabled = self.encryption_enabled.clone();
            let tpm2_row = self.tpm2_row.clone();
            let tpm2_pin_row = self.tpm2_pin_row.clone();
            let tpm2_enabled = self.tpm2_enabled.clone();
            let revalidate = revalidate.clone();
            self.encryption_row
                .connect_enable_expansion_notify(move |row| {
                    let enabled = row.enables_expansion();
                    encryption_enabled.set(enabled);
                    tpm2_row.set_sensitive(enabled && detect_tpm2());
                    if !enabled {
                        tpm2_row.set_active(false);
                        tpm2_enabled.set(false);
                        tpm2_pin_row.set_visible(false);
                    }
                    revalidate();
                });
        }

        {
            let tpm2_enabled = self.tpm2_enabled.clone();
            let tpm2_pin_row = self.tpm2_pin_row.clone();
            let revalidate = revalidate.clone();
            self.tpm2_row.connect_active_notify(move |row| {
                tpm2_enabled.set(row.is_active());
                tpm2_pin_row.set_visible(row.is_active());
                revalidate();
            });
        }

        {
            let revalidate = revalidate.clone();
            self.password_widget.connect_changed(move || revalidate());
        }
    }

    /// Re-lists disks from scratch (not just the selected one's partitions)
    /// and reloads whichever disk was selected before, keeping it selected —
    /// used by the "Refresh" button and after a GParted round-trip, both of
    /// which can change more than partitions (GParted can rewrite the
    /// partition table itself, e.g. MBR → GPT).
    fn build_refresh_disks(
        &self,
        backends: &Backends,
        runtime: &tokio::runtime::Handle,
        load_disk: Rc<dyn Fn(String)>,
    ) -> Rc<dyn Fn()> {
        let disk_backend = backends.disk.clone();
        let runtime = runtime.clone();
        let disks = self.disks.clone();
        let disk_dropdown = self.disk_dropdown.clone();
        let disk_state = self.disk_state.clone();

        Rc::new(move || {
            let keep_path = disk_state.borrow().as_ref().map(|s| s.disk.path.clone());
            let disk_backend = disk_backend.clone();
            let disks = disks.clone();
            let disk_dropdown = disk_dropdown.clone();
            let load_disk = load_disk.clone();
            bridge::spawn(
                &runtime,
                async move { disk_backend.list_disks().await },
                move |result| {
                    let loaded = result.unwrap_or_default();
                    if loaded.is_empty() {
                        return;
                    }
                    let names: Vec<String> = loaded
                        .iter()
                        .map(|d| {
                            format!(
                                "{} ({}{})",
                                d.model,
                                human_bytes(d.size_bytes),
                                if d.is_removable {
                                    format!(", {}", tr("removable"))
                                } else {
                                    String::new()
                                }
                            )
                        })
                        .collect();
                    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
                    let model = gtk::StringList::new(&refs);
                    size_dropdown_to_widest(&disk_dropdown, &model);
                    disk_dropdown.set_model(Some(&model));
                    let target = keep_path
                        .as_ref()
                        .and_then(|p| loaded.iter().position(|d| &d.path == p))
                        .unwrap_or(0);
                    disk_dropdown.set_selected(target as u32);
                    let target_path = loaded[target].path.clone();
                    *disks.borrow_mut() = loaded;
                    load_disk(target_path);
                },
            );
        })
    }

    fn build_load_disk(
        &self,
        backends: &Backends,
        runtime: &tokio::runtime::Handle,
        callbacks: LoadDiskCallbacks,
    ) -> Rc<dyn Fn(String)> {
        let LoadDiskCallbacks {
            revalidate,
            refresh_windows_scale,
            clamp_swap,
            alongside_infeasible,
        } = callbacks;
        let disk_backend = backends.disk.clone();
        let runtime = runtime.clone();
        let disks = self.disks.clone();
        let disk_state = self.disk_state.clone();
        let before_bar = self.before_bar.clone();
        let manual_editor_handle = self.manual_editor.clone();
        let freespace_dropdown = self.freespace_dropdown.clone();
        let selected_gap_id = self.selected_gap_id.clone();
        let mode_rows = self.mode_rows.clone();
        let mode_checks = self.mode_checks.clone();
        let selected_mode = self.selected_mode.clone();
        let ntfs_warning_group = self.ntfs_warning_group.clone();
        let ntfs_warning_label = self.ntfs_warning_label.clone();
        let ntfs_blocker = self.ntfs_blocker.clone();
        let ram_bytes = self.ram_bytes.clone();
        let uefi = self.uefi;

        Rc::new(move |disk_path: String| {
            let Some(disk) = disks.borrow().iter().find(|d| d.path == disk_path).cloned() else {
                return;
            };
            let disk_backend = disk_backend.clone();
            let disk_state_slot = disk_state.clone();
            let before_bar = before_bar.clone();
            let manual_editor_handle = manual_editor_handle.clone();
            let freespace_dropdown = freespace_dropdown.clone();
            let selected_gap_id = selected_gap_id.clone();
            let mode_rows = mode_rows.clone();
            let mode_checks = mode_checks.clone();
            let selected_mode = selected_mode.clone();
            let ntfs_warning_group = ntfs_warning_group.clone();
            let ntfs_warning_label = ntfs_warning_label.clone();
            let ntfs_blocker = ntfs_blocker.clone();
            let ram_bytes = ram_bytes.clone();
            let revalidate = revalidate.clone();
            let refresh_windows_scale = refresh_windows_scale.clone();
            let clamp_swap = clamp_swap.clone();
            let alongside_infeasible = alongside_infeasible.clone();
            let runtime_inner = runtime.clone();
            let disk_path_for_usage = disk_path.clone();

            bridge::spawn(
                &runtime,
                {
                    let disk_backend = disk_backend.clone();
                    let disk_path = disk_path.clone();
                    async move { disk_backend.list_partitions(&disk_path).await }
                },
                move |result| {
                    let partitions = result.unwrap_or_default();
                    let gaps = layout::free_gaps(disk.size_bytes, &partitions);
                    let ntfs_path = partitions
                        .iter()
                        .find(|p| p.fs_type.as_deref() == Some("ntfs"))
                        .map(|p| p.path.clone());

                    let disk2 = disk.clone();
                    let disk_state_slot2 = disk_state_slot.clone();
                    let before_bar2 = before_bar.clone();
                    let manual_editor_handle2 = manual_editor_handle.clone();
                    let freespace_dropdown2 = freespace_dropdown.clone();
                    let selected_gap_id2 = selected_gap_id.clone();
                    let mode_rows2 = mode_rows.clone();
                    let mode_checks2 = mode_checks.clone();
                    let selected_mode2 = selected_mode.clone();
                    let ntfs_warning_group2 = ntfs_warning_group.clone();
                    let ntfs_warning_label2 = ntfs_warning_label.clone();
                    let ntfs_blocker2 = ntfs_blocker.clone();
                    let ram_bytes2 = ram_bytes.clone();
                    let revalidate2 = revalidate.clone();
                    let refresh_windows_scale2 = refresh_windows_scale.clone();
                    let clamp_swap2 = clamp_swap.clone();
                    let partitions2 = partitions.clone();
                    let gaps2 = gaps.clone();

                    let disk_backend_for_usage = disk_backend.clone();
                    let runtime_for_usage = runtime_inner.clone();
                    let alongside_infeasible2 = alongside_infeasible.clone();
                    let finish = move |ntfs: Option<(String, NtfsProbe)>| {
                        let state = DiskState {
                            disk: disk2.clone(),
                            partitions: partitions2.clone(),
                            gaps: gaps2.clone(),
                            ntfs: ntfs.clone(),
                        };

                        let gap_names: Vec<String> = state
                            .gaps
                            .iter()
                            .map(|g| {
                                format!(
                                    "{} {} {} {}",
                                    human_bytes(g.size_bytes),
                                    tr("free at"),
                                    human_bytes(g.start_bytes),
                                    tr("from the start of the disk")
                                )
                            })
                            .collect();
                        let refs: Vec<&str> = gap_names.iter().map(String::as_str).collect();
                        let model = gtk::StringList::new(&refs);
                        size_dropdown_to_widest(&freespace_dropdown2, &model);
                        freespace_dropdown2.set_model(Some(&model));
                        let biggest_idx = state
                            .gaps
                            .iter()
                            .enumerate()
                            .max_by_key(|(_, g)| g.size_bytes)
                            .map(|(idx, _)| idx as u32);
                        *selected_gap_id2.borrow_mut() = biggest_idx
                            .and_then(|idx| state.gaps.get(idx as usize))
                            .map(|g| g.id.clone());
                        if let Some(idx) = biggest_idx {
                            freespace_dropdown2.set_selected(idx);
                        }

                        manual_editor_handle2.set_data(&state.partitions, &state.gaps);
                        before_bar2.set_segments(before_segments(&state));

                        let mut alongside_disabled = false;
                        let mut ntfs_blocker_now: Option<NtfsBlocker> = None;
                        for (row, mode) in mode_rows2.iter().zip(MODES) {
                            match mode_feasibility(mode, &state, ram_bytes2.get(), uefi) {
                                Ok(()) => {
                                    row.set_sensitive(true);
                                    row.set_subtitle("");
                                    if mode == PartitionMode::AlongsideWindows {
                                        *alongside_infeasible2.borrow_mut() = None;
                                    }
                                }
                                // Any `NtfsBlocked` or `NoWindows` blocker
                                // greys the row out — the action message
                                // moves into the warning strip for
                                // `NtfsBlocked`, stays the subtitle for
                                // `NoWindows`. Every other blocker
                                // (`ShrinkTooSmall` etc.) keeps the row
                                // clickable; clicking it raises a popup (see
                                // the mode-check handler) instead, the only
                                // way "not enough space on Windows" is ever
                                // reachable.
                                Err(e) if mode == PartitionMode::AlongsideWindows => {
                                    match alongside_row_state(&e) {
                                        AlongsideRowState::Disabled(Some(b)) => {
                                            row.set_sensitive(false);
                                            row.set_subtitle("");
                                            *alongside_infeasible2.borrow_mut() = None;
                                            alongside_disabled = true;
                                            ntfs_blocker_now = Some(b);
                                        }
                                        AlongsideRowState::Disabled(None) => {
                                            row.set_sensitive(false);
                                            row.set_subtitle(&e.msgid());
                                            *alongside_infeasible2.borrow_mut() = None;
                                            alongside_disabled = true;
                                        }
                                        AlongsideRowState::Clickable => {
                                            row.set_sensitive(true);
                                            row.set_subtitle(&e.msgid());
                                            *alongside_infeasible2.borrow_mut() = Some(e);
                                        }
                                    }
                                }
                                Err(e) => {
                                    row.set_sensitive(false);
                                    row.set_subtitle(&e.msgid());
                                }
                            }
                        }

                        ntfs_blocker2.set(ntfs_blocker_now);
                        match ntfs_blocker_now {
                            Some(b) => {
                                ntfs_warning_label2.set_label(&PlanError::NtfsBlocked(b).msgid());
                                ntfs_warning_group2.set_visible(true);
                            }
                            None => ntfs_warning_group2.set_visible(false),
                        }

                        // If `AlongsideWindows` was selected and just became
                        // disabled (disk swap via the dropdown, or a
                        // "Refresh"), the `CheckButton` would otherwise stay
                        // active on a greyed-out row. Reactivate the first
                        // mode outside of `mode_change_guard` so the
                        // existing `toggled` handler puts
                        // `detail_stack`/`swap_group`/`revalidate()` back in
                        // sync.
                        if alongside_disabled
                            && selected_mode2.get() == PartitionMode::AlongsideWindows
                        {
                            mode_checks2[0].set_active(true);
                        }

                        *disk_state_slot2.borrow_mut() = Some(state);
                        clamp_swap2();
                        refresh_windows_scale2();
                        revalidate2();

                        // Two-phase render: the layout above is already on
                        // screen, usage stripes fill in once the probes
                        // (cheap, but not free — `dumpe2fs`/`ntfsresize`)
                        // come back.
                        let disk_backend = disk_backend_for_usage.clone();
                        let disk_path = disk_path_for_usage.clone();
                        let disk_state_slot3 = disk_state_slot2.clone();
                        let before_bar3 = before_bar2.clone();
                        let revalidate3 = revalidate2.clone();
                        let partitions_for_usage = partitions2.clone();
                        let ntfs_for_usage = ntfs.clone();
                        bridge::spawn(
                            &runtime_for_usage,
                            async move {
                                let mut results = Vec::with_capacity(partitions_for_usage.len());
                                for p in &partitions_for_usage {
                                    let used = if let Some((ntfs_path, probe)) = &ntfs_for_usage
                                        && &p.path == ntfs_path
                                    {
                                        probe.used_bytes
                                    } else {
                                        disk_backend
                                            .probe_usage(&p.path, p.fs_type.as_deref())
                                            .await
                                            .unwrap_or(None)
                                    };
                                    results.push((p.path.clone(), used));
                                }
                                results
                            },
                            move |results| {
                                // Guard against a stale result — the user
                                // may have switched disks while the probes
                                // were running.
                                let mut state_ref = disk_state_slot3.borrow_mut();
                                let Some(state) = state_ref.as_mut() else {
                                    return;
                                };
                                if state.disk.path != disk_path {
                                    return;
                                }
                                for (path, used) in results {
                                    if let Some(p) =
                                        state.partitions.iter_mut().find(|p| p.path == path)
                                    {
                                        p.used_bytes = used;
                                    }
                                }
                                before_bar3.set_segments(before_segments(state));
                                drop(state_ref);
                                revalidate3();
                            },
                        );
                    };

                    if let Some(path) = ntfs_path {
                        bridge::spawn(
                            &runtime_inner,
                            {
                                let disk_backend = disk_backend.clone();
                                let path = path.clone();
                                async move { disk_backend.probe_ntfs(&path).await }
                            },
                            move |probe_result| {
                                finish(probe_result.ok().map(|probe| (path.clone(), probe)));
                            },
                        );
                    } else {
                        finish(None);
                    }
                },
            );
        })
    }

    /// Single point of truth for the alongside-Windows slider's range and
    /// label — reads `disk_state` + `selected_swap` + `ram_bytes` and goes
    /// through `plan::shrink_bounds`, so the slider can never offer a
    /// position `plan()` itself would reject. Called after every input that
    /// can change the bounds (disk load, swap-mode change, RAM detection)
    /// and once more from `retranslate()` so `windows_label`/`windows_banner`
    /// don't go stale on a language switch.
    fn build_refresh_windows_scale(&self) -> Rc<dyn Fn()> {
        let disk_state = self.disk_state.clone();
        let selected_swap = self.selected_swap.clone();
        let ram_bytes = self.ram_bytes.clone();
        let uefi = self.uefi;
        let windows_scale = self.windows_scale.clone();
        let windows_label = self.windows_label.clone();
        let windows_banner = self.windows_banner.clone();
        let shrink_to_bytes = self.shrink_to_bytes.clone();
        let windows_ntfs_size = self.windows_ntfs_size.clone();

        Rc::new(move || {
            let state = disk_state.borrow();
            let Some(state) = state.as_ref() else {
                return;
            };
            let Some((ntfs_path, probe)) = &state.ntfs else {
                windows_banner.set_revealed(false);
                return;
            };
            if let Some(blocker) = probe.blockers.first() {
                windows_scale.set_sensitive(false);
                windows_banner.set_title(&PlanError::NtfsBlocked(*blocker).msgid());
                windows_banner.set_revealed(true);
                return;
            }

            let ntfs_size = state
                .partitions
                .iter()
                .find(|p| &p.path == ntfs_path)
                .map(|p| p.size_bytes)
                .unwrap_or(probe.current_size_bytes);
            windows_ntfs_size.set(ntfs_size);

            let swap_bytes =
                layout::align_up(compute_swap_bytes(selected_swap.get(), ram_bytes.get()));

            match plan::shrink_bounds(ntfs_size, probe, swap_bytes, uefi) {
                Some(bounds) => {
                    windows_scale.set_sensitive(true);
                    windows_banner.set_revealed(false);
                    windows_scale.set_range(bounds.min as f64, bounds.max as f64);
                    let current = shrink_to_bytes.get().unwrap_or(bounds.min);
                    let value = if (bounds.min..=bounds.max).contains(&current) {
                        current
                    } else {
                        bounds.min + (bounds.max - bounds.min) / 2
                    };
                    windows_scale.set_value(value as f64);
                    shrink_to_bytes.set(Some(value));
                    let esp_bytes = if uefi { plan::NEW_ESP_BYTES } else { 0 };
                    set_windows_label(&windows_label, ntfs_size, value, esp_bytes, swap_bytes);
                }
                None => {
                    windows_scale.set_sensitive(false);
                    shrink_to_bytes.set(None);
                    windows_banner.set_title(&PlanError::ShrinkTooSmall.msgid());
                    windows_banner.set_revealed(true);
                }
            }
        })
    }

    fn build_revalidate(&self) -> Rc<dyn Fn()> {
        let disk_state = self.disk_state.clone();
        let selected_mode = self.selected_mode.clone();
        let ram_bytes = self.ram_bytes.clone();
        let uefi = self.uefi;
        let validity = self.validity.clone();
        let after_bar = self.after_bar.clone();
        let warning_banner = self.warning_banner.clone();

        let step_for_cfg = PartitioningStepCfgHandle {
            selected_mode: self.selected_mode.clone(),
            selected_swap: self.selected_swap.clone(),
            disk_state: self.disk_state.clone(),
            shrink_to_bytes: self.shrink_to_bytes.clone(),
            selected_gap_id: self.selected_gap_id.clone(),
            encryption_enabled: self.encryption_enabled.clone(),
            encryption_passphrase: self.password_widget.clone(),
            tpm2_enabled: self.tpm2_enabled.clone(),
            tpm2_pin_row: self.tpm2_pin_row.clone(),
            manual_editor: self.manual_editor.clone(),
        };

        Rc::new(move || {
            let _ = &selected_mode;
            let _ = &ram_bytes;
            let cfg = step_for_cfg.snapshot();
            let Some(state) = disk_state.borrow().clone() else {
                validity.set_blocked(tr("Select a target disk"));
                after_bar.set_segments(vec![]);
                warning_banner.set_revealed(false);
                return;
            };

            // Checked ahead of `plan()` so it can be reported alongside a
            // still-populated preview instead of blanking it — a missing
            // passphrase isn't a partitioning constraint, see
            // `engine::plan::validate_config`.
            let config_validation =
                plan::validate_config(&cfg, step_for_cfg.encryption_passphrase.is_valid());

            let input = PlanInput {
                disk: state.disk.clone(),
                partitions: state.partitions.clone(),
                gaps: state.gaps.clone(),
                ntfs: state.ntfs.clone(),
                cfg,
                ram_bytes: ram_bytes.get(),
                uefi,
            };

            match plan::plan(&input) {
                Ok(result) => {
                    let segments: Vec<DiskBarSegment> = result
                        .preview
                        .iter()
                        .map(|s| DiskBarSegment {
                            label: s.label.clone(),
                            size_bytes: s.size_bytes,
                            role: preview_role_to_bar_role(s.role, &s.fs_type),
                            encrypted: s.encrypted,
                            used_bytes: s.used_bytes,
                        })
                        .collect();
                    after_bar.set_segments(segments);

                    match config_validation {
                        Ok(()) => {
                            validity.set_ready();
                            let warnings = plan::warnings(&input);
                            if warnings.contains(&PlanWarning::RemovableTarget) {
                                warning_banner.set_title(&tr(
                                    "This disk is removable — make sure it isn't your install medium",
                                ));
                                warning_banner.set_revealed(true);
                            } else {
                                warning_banner.set_revealed(false);
                            }
                        }
                        Err(e) => {
                            validity.set_blocked(e.msgid());
                            warning_banner.set_revealed(false);
                        }
                    }
                }
                Err(e) => {
                    validity.set_blocked(e.msgid());
                    after_bar.set_segments(vec![]);
                    warning_banner.set_revealed(false);
                }
            }
        })
    }

    /// Auto-clamps the swap selection when it doesn't fit the current
    /// mode/disk — walks `SWAP_DOWNWARD` starting at the current selection,
    /// picks the first mode `plan::plan` accepts (best-case shrink in
    /// `AlongsideWindows`, so a mode the slider could still reach never gets
    /// clamped away), and reveals `swap_banner` if that differs from the
    /// selection. Called from the swap dropdown handler, the mode toggle
    /// handler, and after a disk (re)load — deliberately not from
    /// `revalidate` (runs on every keystroke) or the slider (`shrink_bounds`
    /// already deducts the current swap, so every slider position is
    /// feasible by construction).
    fn build_clamp_swap(&self) -> Rc<dyn Fn()> {
        let disk_state = self.disk_state.clone();
        let selected_swap = self.selected_swap.clone();
        let ram_bytes = self.ram_bytes.clone();
        let uefi = self.uefi;
        let swap_dropdown = self.swap_dropdown.clone();
        let swap_banner = self.swap_banner.clone();
        let reentrant = Rc::new(Cell::new(false));

        let step_for_cfg = PartitioningStepCfgHandle {
            selected_mode: self.selected_mode.clone(),
            selected_swap: self.selected_swap.clone(),
            disk_state: self.disk_state.clone(),
            shrink_to_bytes: self.shrink_to_bytes.clone(),
            selected_gap_id: self.selected_gap_id.clone(),
            encryption_enabled: self.encryption_enabled.clone(),
            encryption_passphrase: self.password_widget.clone(),
            tpm2_enabled: self.tpm2_enabled.clone(),
            tpm2_pin_row: self.tpm2_pin_row.clone(),
            manual_editor: self.manual_editor.clone(),
        };

        Rc::new(move || {
            if reentrant.get() {
                return;
            }
            let state = disk_state.borrow();
            let Some(state) = state.as_ref() else {
                return;
            };

            let current = selected_swap.get();
            let start = SWAP_DOWNWARD
                .iter()
                .position(|m| *m == current)
                .unwrap_or(0);
            let base_cfg = step_for_cfg.snapshot();

            let winner = SWAP_DOWNWARD[start..].iter().find_map(|&mode| {
                let mut cfg = base_cfg.clone();
                cfg.swap_mode = mode;
                if cfg.mode == PartitionMode::AlongsideWindows {
                    cfg.shrink_to_bytes = None;
                }
                let input = PlanInput {
                    disk: state.disk.clone(),
                    partitions: state.partitions.clone(),
                    gaps: state.gaps.clone(),
                    ntfs: state.ntfs.clone(),
                    cfg,
                    ram_bytes: ram_bytes.get(),
                    uefi,
                };
                plan::plan(&input).is_ok().then_some(mode)
            });

            match winner {
                Some(mode) if mode != current => {
                    selected_swap.set(mode);
                    reentrant.set(true);
                    if let Some(idx) = SWAP_MODES.iter().position(|m| *m == mode) {
                        swap_dropdown.set_selected(idx as u32);
                    }
                    reentrant.set(false);
                    swap_banner.set_revealed(true);
                }
                _ => swap_banner.set_revealed(false),
            }
        })
    }
}

/// Bundles the `Rc`/`Cell` handles `build_revalidate`'s closure needs to
/// rebuild a `PartitioningConfig` snapshot on every keystroke, without
/// borrowing `self` (which the closure can't, since it outlives `new()`).
struct PartitioningStepCfgHandle {
    selected_mode: Rc<Cell<PartitionMode>>,
    selected_swap: Rc<Cell<SwapMode>>,
    disk_state: Rc<RefCell<Option<DiskState>>>,
    shrink_to_bytes: Rc<Cell<Option<u64>>>,
    selected_gap_id: Rc<RefCell<Option<String>>>,
    encryption_enabled: Rc<Cell<bool>>,
    encryption_passphrase: PasswordConfirmEntry,
    tpm2_enabled: Rc<Cell<bool>>,
    tpm2_pin_row: adw::PasswordEntryRow,
    manual_editor: crate::widgets::PartitionEditor,
}

impl PartitioningStepCfgHandle {
    fn snapshot(&self) -> PartitioningConfig {
        PartitioningConfig {
            mode: self.selected_mode.get(),
            target_disk: self
                .disk_state
                .borrow()
                .as_ref()
                .map(|s| s.disk.path.clone()),
            shrink_to_bytes: self.shrink_to_bytes.get(),
            selected_free_space_id: self.selected_gap_id.borrow().clone(),
            swap_mode: self.selected_swap.get(),
            encryption_enabled: self.encryption_enabled.get(),
            encryption_passphrase: self.encryption_passphrase.password(),
            tpm2_enabled: self.tpm2_enabled.get(),
            tpm2_pin: self
                .tpm2_enabled
                .get()
                .then(|| self.tpm2_pin_row.text().to_string())
                .filter(|s| !s.is_empty()),
            manual: self.manual_editor.entries(),
        }
    }
}

impl Step for PartitioningStep {
    fn id(&self) -> StepId {
        StepId::Partitioning
    }

    fn title(&self) -> String {
        tr("Partitioning")
    }

    fn icon_name(&self) -> &'static str {
        "partitioning-symbolic"
    }

    fn widget(&self) -> gtk::Widget {
        self.widget.clone()
    }

    fn validity(&self) -> ValidityTracker {
        self.validity.clone()
    }

    fn commit(&self, cfg: &mut InstallConfig) -> mx::Result<()> {
        cfg.partitioning = self.snapshot_cfg();
        Ok(())
    }

    fn retranslate(&self) {
        self.disk_label.set_label(&tr("Target disk"));
        self.refresh_button.set_tooltip_text(Some(&tr("Refresh")));
        self.toggle_before.set_label(Some(&tr("Current layout")));
        self.toggle_after.set_label(Some(&tr("After installation")));
        self.mode_group.set_title(&tr("Installation type"));
        for (row, mode) in self.mode_rows.iter().zip(MODES) {
            row.set_title(&mode_title(mode));
        }
        self.detail_group.set_title(&tr("Options"));
        self.freespace_row.set_title(&tr("Free space region"));
        self.manual_banner.set_title(&tr(
            "Assign existing partitions or create new ones in free space below. To resize or delete an existing partition, use GParted.",
        ));
        self.gparted_label.set_label(&tr("Partition with GParted…"));
        if self.is_fake {
            self.gparted_button
                .set_tooltip_text(Some(&tr("Unavailable with simulated disks")));
        }
        self.swap_group.set_title(&tr("Swap"));
        self.swap_row.set_title(&tr("Swap"));
        let swap_selected = self.swap_dropdown.selected();
        let swap_model = swap_string_list();
        size_dropdown_to_widest(&self.swap_dropdown, &swap_model);
        self.swap_dropdown.set_model(Some(&swap_model));
        self.swap_dropdown.set_selected(swap_selected);
        self.swap_banner.set_title(&tr(
            "Hibernation swap doesn't fit — using standard swap instead",
        ));
        set_help_text(
            &self.swap_help,
            &tr(
                "Disk space used as extra memory when RAM is full. Hibernation also needs it to save the session to disk.",
            ),
        );
        self.encryption_group.set_title(&tr("Encryption"));
        self.encryption_row.set_title(&tr("Encrypt disk"));
        self.encryption_row
            .set_subtitle(&tr("LUKS2, unlocked via TPM2 when available"));
        set_help_text(
            &self.encryption_help,
            &tr(
                "Encrypts the whole disk with LUKS2. A passphrase is required at every boot unless TPM2 unlocking is enabled.",
            ),
        );
        self.password_widget.retranslate();
        self.tpm2_row.set_title(&tr("Use TPM2"));
        set_help_text(
            &self.tpm2_help,
            &tr(
                "The TPM2 chip stores the key and unlocks the disk automatically at boot. Add a PIN to require a code as well.",
            ),
        );
        if !detect_tpm2() {
            self.tpm2_row.set_subtitle(&tr("No TPM2 chip detected"));
        } else {
            self.tpm2_row
                .set_subtitle(&tr("Unlock automatically without a passphrase"));
        }
        self.tpm2_pin_row.set_title(&tr("TPM2 PIN (optional)"));
        if let Some(b) = self.ntfs_blocker.get() {
            self.ntfs_warning_label
                .set_label(&PlanError::NtfsBlocked(b).msgid());
        }
        self.before_bar.retranslate();
        self.after_bar.retranslate();
        self.manual_editor.retranslate();
        self.build_refresh_windows_scale()();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ntfs_blocked_disables_with_warning() {
        for blocker in [
            NtfsBlocker::BitLocker,
            NtfsBlocker::Hibernated,
            NtfsBlocker::DirtyVolume,
            NtfsBlocker::ResizeUnsupported,
        ] {
            assert_eq!(
                alongside_row_state(&PlanError::NtfsBlocked(blocker)),
                AlongsideRowState::Disabled(Some(blocker)),
            );
        }
    }

    #[test]
    fn no_windows_disables_without_warning() {
        assert_eq!(
            alongside_row_state(&PlanError::NoWindows),
            AlongsideRowState::Disabled(None),
        );
    }

    #[test]
    fn other_errors_stay_clickable() {
        for err in [
            PlanError::ShrinkTooSmall,
            PlanError::MbrNeedsConversion,
            PlanError::StaleLayout,
        ] {
            assert_eq!(alongside_row_state(&err), AlongsideRowState::Clickable);
        }
    }
}
