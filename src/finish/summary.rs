//! Final review screen — recaps every answer, recomputes `engine::plan::plan`
//! against the *live* disk state one last time (never trusts a stale plan
//! from whenever step 7 was last visited), and gates the destructive
//! "Install" action behind a confirmation dialog that explicitly lists every
//! existing partition the plan touches: the ones `plan.erases` destroys
//! (deleted, or reformatted in place) and — separately, since their data
//! survives — the ones `PlanOp::ShrinkNtfs` shrinks. Both lists are built by
//! `disk_changes`, in disk order and with the label, filesystem and size of
//! each partition, so an entry carrying no filesystem (the Windows MSR) is a
//! legible row rather than a hole in the numbering.

use crate::backend::Backends;
use crate::backend::disk::{PartitionInfo, short_device_name};
use crate::config::{AppSelection, InstallConfig, PartitionMode, SwapMode};
use crate::engine::live_input::{detect_uefi, gather_plan_input};
use crate::engine::plan::{self, PartitionPlan, PlanError, PlanInput, PlanOp};
use crate::finish::progress::ProgressPage;
use crate::i18n::tr;
use crate::steps::app_pack::{card_title, variant_label};
use crate::steps::card_layout::{
    CARD_SPACING, PAGE_MARGIN, column_min_width, grid_width, vertical_scroller,
};
use crate::widgets::disk_bar::human_bytes;
use crate::{bridge, mx};
use adw::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;

/// Narrowest readable width for a column of `adw::ActionRow`s, in pixels.
const COLUMN_MIN_WIDTH: i32 = 360;
/// Per-column ceiling, in pixels — near Adwaita's 580 px preferences clamp.
const COLUMN_MAX_WIDTH: i32 = 560;

/// Flips the recap from one stacked column to two side-by-side ones from
/// `column_min_width(2, COLUMN_MIN_WIDTH)` pixels up.
///
/// The stacked layout is the built state, so the single breakpoint only ever
/// widens: below it the two blocks follow each other and the page scrolls.
/// `homogeneous` is part of the switch because a `gtk::Box` distributes space
/// along its own orientation — equal widths side by side, but equal *heights*
/// once stacked, which is not wanted.
///
/// # Parameters
/// * `bin` - the `adw::BreakpointBin` whose width drives the condition.
/// * `columns` - the box holding the two column boxes.
/// * `clamp` - the `adw::Clamp` around the page content, whose maximum size is
///   widened to two columns.
///
/// # Post-conditions
/// `bin` carries one `MinWidth` breakpoint laying `columns` out horizontally
/// and homogeneously and widening `clamp`.
fn add_two_column_breakpoint(bin: &adw::BreakpointBin, columns: &gtk::Box, clamp: &adw::Clamp) {
    let breakpoint = adw::Breakpoint::new(adw::BreakpointCondition::new_length(
        adw::BreakpointConditionLengthType::MinWidth,
        f64::from(column_min_width(2, COLUMN_MIN_WIDTH)),
        adw::LengthUnit::Px,
    ));
    breakpoint.add_setter(
        columns,
        "orientation",
        Some(&gtk::Orientation::Horizontal.to_value()),
    );
    breakpoint.add_setter(columns, "homogeneous", Some(&true.to_value()));
    breakpoint.add_setter(
        clamp,
        "maximum-size",
        Some(&grid_width(2, COLUMN_MAX_WIDTH).to_value()),
    );
    bin.add_breakpoint(breakpoint);
}

fn mode_label(mode: PartitionMode) -> String {
    match mode {
        PartitionMode::EntireDisk => tr("Erase entire disk"),
        PartitionMode::AlongsideWindows => tr("Install alongside Windows"),
        PartitionMode::FreeSpace => tr("Use free space"),
        PartitionMode::Manual => tr("Manual"),
    }
}

/// The application step's answer as one line.
///
/// # Parameters
/// * `selection` - the ticked cards.
///
/// # Returns
/// The ticked cards' names in card order, comma-joined, or `No applications`
/// when nothing is ticked — a bare system is a legitimate choice, not a
/// missing answer. A card offering several editions carries the picked one in
/// parentheses, since that is what decides which module gets enabled.
fn apps_label(selection: &AppSelection) -> String {
    if selection.is_empty() {
        return tr("No applications");
    }
    let names: Vec<String> = selection
        .cards()
        .map(|card| {
            let title = card_title(card);
            match selection.variant_of(card) {
                Some(edition) => format!("{title} ({})", variant_label(card, edition.key)),
                None => title,
            }
        })
        .collect();
    names.join(", ")
}
fn swap_label(mode: SwapMode) -> String {
    match mode {
        SwapMode::None => tr("No swap"),
        SwapMode::Standard => tr("Standard swap"),
        SwapMode::Hibernation => tr("Swap with hibernation support"),
    }
}

fn render_backend_error(e: &mx::Error) -> String {
    match e {
        mx::Error::Backend(msgid) => tr(msgid),
        other => other.to_string(),
    }
}

/// `--fake-disks=`/`--fake-error=` runs a real pipeline against fake
/// backends — no actual disk is touched — so the confirmation dialog must
/// say so plainly instead of threatening data loss that cannot happen. Never
/// translated: it only ever appears on a developer's own machine. `None` in
/// a release build, where `debugsim` does not exist at all.
#[cfg(debug_assertions)]
fn fake_dialog_text() -> Option<(String, String)> {
    let sim = crate::debugsim::active()?;
    Some((
        "SIMULATION — run a fake install?".to_string(),
        format!(
            "Disk scenario(s): {}\nInjected fault: {}\n\nNo real disk will be touched.",
            sim.scenario_names().join(", "),
            sim.fault_name(),
        ),
    ))
}
#[cfg(not(debug_assertions))]
fn fake_dialog_text() -> Option<(String, String)> {
    None
}

/// One change the plan makes to a partition that already exists — what the
/// review page's two destructive lists are built from.
#[derive(Clone)]
struct DiskChange {
    /// Short device name (`nvme0n1p3`), never the udisks2 object path the
    /// backend keys partitions on.
    name: String,
    /// Label, filesystem and size, rendered for display in the current
    /// language.
    detail: String,
}

/// Describes a partition about to be deleted or reformatted.
///
/// * `part` - the live partition, as enumerated into `PlanInput::partitions`.
///
/// # Returns
/// Its label (when it has one), filesystem — or `No filesystem`, so a
/// metadata partition such as the Windows MSR reads as the harmless entry it
/// is rather than as a missing line — and size, comma-joined.
fn erase_detail(part: &PartitionInfo) -> String {
    let mut parts = Vec::new();
    if let Some(label) = &part.label {
        parts.push(label.clone());
    }
    parts.push(match &part.fs_type {
        Some(fs) => fs.to_uppercase(),
        None => tr("No filesystem"),
    });
    parts.push(human_bytes(part.size_bytes));
    parts.join(", ")
}

/// Describes a partition about to be shrunk in place.
///
/// * `part` - the live partition, as enumerated into `PlanInput::partitions`.
/// * `new_size_bytes` - size the plan shrinks it to.
///
/// # Returns
/// Its label (when it has one), filesystem and the size transition, so the
/// row says what is kept rather than only what changes.
fn resize_detail(part: &PartitionInfo, new_size_bytes: u64) -> String {
    let mut parts = Vec::new();
    if let Some(label) = &part.label {
        parts.push(label.clone());
    }
    if let Some(fs) = &part.fs_type {
        parts.push(fs.to_uppercase());
    }
    parts.push(format!(
        "{} → {}",
        human_bytes(part.size_bytes),
        human_bytes(new_size_bytes)
    ));
    parts.join(", ")
}

/// Splits what the plan does to the disk's existing partitions into the two
/// lists the review page shows.
///
/// Both are built by walking `input.partitions` in **disk order** and keeping
/// the ones the plan touches, rather than by walking the plan: the backend
/// enumeration follows no particular order (`Udisks2Backend::list_partitions`
/// iterates D-Bus objects), and a list of what is about to be destroyed has
/// to read left to right like the disk bar above it.
///
/// * `input` - the input [`plan::plan`] was called with.
/// * `result` - its result.
///
/// # Returns
/// `(erases, resizes)` — the partitions deleted or reformatted in place
/// (`PartitionPlan::erases`), then the ones shrunk in place
/// (`PlanOp::ShrinkNtfs`), each as display rows.
fn disk_changes(input: &PlanInput, result: &PartitionPlan) -> (Vec<DiskChange>, Vec<DiskChange>) {
    let mut ordered: Vec<&PartitionInfo> = input.partitions.iter().collect();
    ordered.sort_by_key(|p| p.start_bytes);

    let erases = ordered
        .iter()
        .filter(|p| result.erases.contains(&p.path))
        .map(|p| DiskChange {
            name: short_device_name(&p.path).to_string(),
            detail: erase_detail(p),
        })
        .collect();

    let resizes = ordered
        .iter()
        .filter_map(|p| {
            result.ops.iter().find_map(|op| match op {
                PlanOp::ShrinkNtfs {
                    path,
                    new_size_bytes,
                } if path == &p.path => Some(DiskChange {
                    name: short_device_name(&p.path).to_string(),
                    detail: resize_detail(p, *new_size_bytes),
                }),
                _ => None,
            })
        })
        .collect();

    (erases, resizes)
}

/// Repopulates one of the review page's change groups.
///
/// * `group` - the `adw::PreferencesGroup` to fill; hidden when `changes` is
///   empty, since an empty group still draws its title.
/// * `rows` - the rows this group currently holds, drained and refilled so a
///   second `refresh` cannot stack a stale list under a fresh one.
/// * `changes` - the rows to show, in display order.
///
/// # Post-conditions
/// `group` holds exactly one `adw::ActionRow` per entry of `changes`, `rows`
/// tracks them, and `group`'s visibility matches `!changes.is_empty()`.
fn fill_change_group(
    group: &adw::PreferencesGroup,
    rows: &Rc<RefCell<Vec<adw::ActionRow>>>,
    changes: &[DiskChange],
) {
    for row in rows.borrow_mut().drain(..) {
        group.remove(&row);
    }
    for change in changes {
        let row = adw::ActionRow::builder()
            .title(&change.name)
            .subtitle(&change.detail)
            .build();
        group.add(&row);
        rows.borrow_mut().push(row);
    }
    group.set_visible(!changes.is_empty());
}

/// Logs what the two change lists came out as, on a debug build only.
///
/// The review page has no other diagnostic channel — a group that stays
/// hidden because its list is empty looks exactly like a group that failed to
/// render — so a `cargo run` prints the target, the mode, how many partitions
/// were enumerated and what each list holds. A release binary (the ISO's)
/// prints nothing.
///
/// * `input` - the input the plan was computed from.
/// * `erases` - partitions about to be destroyed.
/// * `resizes` - partitions about to be shrunk.
#[cfg(debug_assertions)]
fn trace_changes(input: &PlanInput, erases: &[DiskChange], resizes: &[DiskChange]) {
    let names = |changes: &[DiskChange]| {
        changes
            .iter()
            .map(|c| c.name.clone())
            .collect::<Vec<_>>()
            .join(" ")
    };
    eprintln!(
        "summary: disk={} mode={:?} partitions={} erases=[{}] resizes=[{}]",
        input.disk.path,
        input.cfg.mode,
        input.partitions.len(),
        names(erases),
        names(resizes),
    );
}
#[cfg(not(debug_assertions))]
fn trace_changes(_input: &PlanInput, _erases: &[DiskChange], _resizes: &[DiskChange]) {}

/// Renders one [`DiskChange`] list as the dialog's bulleted paragraph.
///
/// * `heading` - already-translated line introducing the list.
/// * `changes` - the rows to bullet.
///
/// # Returns
/// A blank line, the heading, then one `• name — detail` bullet per change.
fn change_paragraph(heading: &str, changes: &[DiskChange]) -> String {
    let bullets: Vec<String> = changes
        .iter()
        .map(|c| format!("• {} — {}", c.name, c.detail))
        .collect();
    format!("\n\n{heading}:\n{}", bullets.join("\n"))
}

#[derive(Clone)]
pub struct SummaryPage {
    page: adw::NavigationPage,
    language_row: adw::ActionRow,
    timezone_row: adw::ActionRow,
    keyboard_row: adw::ActionRow,
    network_row: adw::ActionRow,
    disk_row: adw::ActionRow,
    mode_row: adw::ActionRow,
    swap_row: adw::ActionRow,
    encryption_row: adw::ActionRow,
    user_row: adw::ActionRow,
    desktop_row: adw::ActionRow,
    apps_row: adw::ActionRow,
    overview_group: adw::PreferencesGroup,
    disk_group: adw::PreferencesGroup,
    erases_group: adw::PreferencesGroup,
    resizes_group: adw::PreferencesGroup,
    plan_banner: adw::Banner,
    install_button: gtk::Button,
    spinner: gtk::Spinner,

    backends: Backends,
    runtime: tokio::runtime::Handle,
    nav_view: adw::NavigationView,
    progress_page: ProgressPage,
    pending_erases: Rc<RefCell<Vec<DiskChange>>>,
    pending_resizes: Rc<RefCell<Vec<DiskChange>>>,
    pending_disk_label: Rc<RefCell<String>>,
    pending_config: Rc<RefCell<Option<InstallConfig>>>,
    erase_rows: Rc<RefCell<Vec<adw::ActionRow>>>,
    resize_rows: Rc<RefCell<Vec<adw::ActionRow>>>,
}

impl SummaryPage {
    pub fn new(
        backends: Backends,
        runtime: tokio::runtime::Handle,
        nav_view: adw::NavigationView,
        progress_page: ProgressPage,
    ) -> Self {
        let overview_group = adw::PreferencesGroup::builder()
            .title(tr("Overview"))
            .build();
        let language_row = adw::ActionRow::builder().title(tr("Language")).build();
        let timezone_row = adw::ActionRow::builder().title(tr("Timezone")).build();
        let keyboard_row = adw::ActionRow::builder().title(tr("Keyboard")).build();
        let network_row = adw::ActionRow::builder().title(tr("Network")).build();
        let user_row = adw::ActionRow::builder().title(tr("User account")).build();
        let desktop_row = adw::ActionRow::builder()
            .title(tr("Desktop environment"))
            .build();
        let apps_row = adw::ActionRow::builder()
            .title(tr("Activities"))
            .subtitle_lines(0)
            .build();
        for row in [
            &language_row,
            &timezone_row,
            &keyboard_row,
            &network_row,
            &user_row,
            &desktop_row,
            &apps_row,
        ] {
            overview_group.add(row);
        }

        let disk_group = adw::PreferencesGroup::builder()
            .title(tr("Partitioning"))
            .build();
        let disk_row = adw::ActionRow::builder().title(tr("Target disk")).build();
        let mode_row = adw::ActionRow::builder().title(tr("Mode")).build();
        let swap_row = adw::ActionRow::builder().title(tr("Swap")).build();
        let encryption_row = adw::ActionRow::builder().title(tr("Encryption")).build();
        for row in [&disk_row, &mode_row, &swap_row, &encryption_row] {
            disk_group.add(row);
        }

        let plan_banner = adw::Banner::new("");

        let erases_group = adw::PreferencesGroup::builder()
            .title(tr("This will permanently erase"))
            .build();
        erases_group.set_visible(false);

        let resizes_group = adw::PreferencesGroup::builder()
            .title(tr("This will be resized"))
            .build();
        resizes_group.set_visible(false);

        let spinner = gtk::Spinner::new();
        let install_button = gtk::Button::builder()
            .label(tr("Install"))
            .css_classes(["suggested-action", "pill"])
            .sensitive(false)
            .build();
        // Pinned to the bottom of the page, outside the content clamp, so it
        // sits against the window edge rather than against the recap columns.
        let button_box = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        button_box.set_halign(gtk::Align::End);
        button_box.set_margin_bottom(PAGE_MARGIN);
        button_box.set_margin_start(PAGE_MARGIN);
        button_box.set_margin_end(PAGE_MARGIN);
        button_box.append(&spinner);
        button_box.append(&install_button);

        let left_column = gtk::Box::new(gtk::Orientation::Vertical, CARD_SPACING);
        left_column.set_hexpand(true);
        left_column.append(&overview_group);
        let right_column = gtk::Box::new(gtk::Orientation::Vertical, CARD_SPACING);
        right_column.set_hexpand(true);
        right_column.append(&disk_group);
        right_column.append(&erases_group);
        right_column.append(&resizes_group);

        // Stacked is the built state; `add_two_column_breakpoint` is what makes
        // this horizontal once the window is wide enough for two columns.
        let columns = gtk::Box::new(gtk::Orientation::Vertical, CARD_SPACING);
        columns.append(&left_column);
        columns.append(&right_column);
        // Takes every pixel the install button leaves and centres the recap in
        // it, so a short recap sits mid-page instead of hugging the header.
        columns.set_vexpand(true);
        columns.set_valign(gtk::Align::Center);

        let content = gtk::Box::new(gtk::Orientation::Vertical, CARD_SPACING);
        content.set_margin_top(PAGE_MARGIN);
        content.set_margin_bottom(PAGE_MARGIN);
        content.set_margin_start(PAGE_MARGIN);
        content.set_margin_end(PAGE_MARGIN);
        content.append(&columns);

        let clamp = adw::Clamp::builder()
            .maximum_size(grid_width(1, COLUMN_MAX_WIDTH))
            .tightening_threshold(COLUMN_MIN_WIDTH)
            .vexpand(true)
            .child(&content)
            .build();

        // What gives `columns` spare room to centre in, and the button a
        // bottom to sit at, is `vexpand(true)` on the scrolled window itself:
        // it is the only expanding child of `page_box`, between the banner and
        // the pinned button row. The `vexpand` here and on `clamp` only
        // propagate that flag back down; the viewport inside the scrolled
        // window ignores expand flags entirely and allocates the height
        // `vertical_scroller` computes.
        let bin = adw::BreakpointBin::builder()
            .width_request(COLUMN_MIN_WIDTH + 2 * PAGE_MARGIN)
            .height_request(400)
            .vexpand(true)
            .child(&clamp)
            .build();
        add_two_column_breakpoint(&bin, &columns, &clamp);

        let scrolled = vertical_scroller(&bin);

        // The banner stays edge-to-edge: `adw::Banner` draws its own background
        // across the full width, and the page margins live on `content`.
        let page_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
        page_box.append(&plan_banner);
        page_box.append(&scrolled);
        page_box.append(&button_box);

        // No `HeaderBar` of its own: `app.rs` already wraps the whole
        // `NavigationView` in one, and a second bar only stacks a duplicate row
        // of window controls under the first.
        let page = adw::NavigationPage::with_tag(
            &page_box,
            &tr("Ready to install"),
            crate::finish::SUMMARY_TAG,
        );

        let step = Self {
            page,
            language_row,
            timezone_row,
            keyboard_row,
            network_row,
            disk_row,
            mode_row,
            swap_row,
            encryption_row,
            user_row,
            desktop_row,
            apps_row,
            overview_group,
            disk_group,
            erases_group,
            resizes_group,
            plan_banner,
            install_button,
            spinner,
            backends,
            runtime,
            nav_view,
            progress_page,
            pending_erases: Rc::new(RefCell::new(Vec::new())),
            pending_resizes: Rc::new(RefCell::new(Vec::new())),
            pending_disk_label: Rc::new(RefCell::new(String::new())),
            pending_config: Rc::new(RefCell::new(None)),
            erase_rows: Rc::new(RefCell::new(Vec::new())),
            resize_rows: Rc::new(RefCell::new(Vec::new())),
        };
        step.wire_install_button();
        step
    }

    pub fn page(&self) -> adw::NavigationPage {
        self.page.clone()
    }

    /// Re-applies every static label after a language change.
    ///
    /// The page is built once at startup, before the user has even reached the
    /// language step, so unlike a `Step` it is never rebuilt — without this its
    /// titles stay frozen in the boot locale. Subtitles are not touched here:
    /// [`SummaryPage::refresh`] recomputes them from the config right before
    /// each push.
    ///
    /// # Post-conditions
    /// Page title, group titles, row titles and the install button label are in
    /// the current language. The plan banner is left alone — its text is an
    /// outcome, re-set by the next `refresh`.
    pub fn retranslate(&self) {
        self.page.set_title(&tr("Ready to install"));
        self.overview_group.set_title(&tr("Overview"));
        self.disk_group.set_title(&tr("Partitioning"));
        self.erases_group
            .set_title(&tr("This will permanently erase"));
        self.resizes_group.set_title(&tr("This will be resized"));
        self.language_row.set_title(&tr("Language"));
        self.timezone_row.set_title(&tr("Timezone"));
        self.keyboard_row.set_title(&tr("Keyboard"));
        self.network_row.set_title(&tr("Network"));
        self.user_row.set_title(&tr("User account"));
        self.desktop_row.set_title(&tr("Desktop environment"));
        self.apps_row.set_title(&tr("Activities"));
        self.disk_row.set_title(&tr("Target disk"));
        self.mode_row.set_title(&tr("Mode"));
        self.swap_row.set_title(&tr("Swap"));
        self.encryption_row.set_title(&tr("Encryption"));
        self.install_button.set_label(&tr("Install"));
    }

    fn wire_install_button(&self) {
        let nav_view = self.nav_view.clone();
        let progress_page = self.progress_page.clone();
        let backends = self.backends.clone();
        let runtime = self.runtime.clone();
        let pending_erases = self.pending_erases.clone();
        let pending_resizes = self.pending_resizes.clone();
        let pending_disk_label = self.pending_disk_label.clone();
        let pending_config = self.pending_config.clone();

        self.install_button.connect_clicked(move |button| {
            let Some(cfg) = pending_config.borrow().clone() else {
                return;
            };
            let erases = pending_erases.borrow().clone();
            let resizes = pending_resizes.borrow().clone();
            let disk_label = pending_disk_label.borrow().clone();

            let is_fake = fake_dialog_text().is_some();
            let (heading, body, install_label) = match fake_dialog_text() {
                Some((heading, body)) => (heading, body, "Simulate".to_string()),
                None => {
                    let heading = tr("Erase disk and install Modulix OS?");
                    let mut body = format!(
                        "{}\n\n{}: {}",
                        tr("This cannot be undone."),
                        tr("Target disk"),
                        disk_label
                    );
                    if erases.is_empty() {
                        body.push_str(&format!(
                            "\n\n{}",
                            tr("No existing partitions will be deleted.")
                        ));
                    } else {
                        body.push_str(&change_paragraph(
                            &tr("The following partitions will be permanently erased"),
                            &erases,
                        ));
                    }
                    if !resizes.is_empty() {
                        body.push_str(&change_paragraph(
                            &tr("The following partitions will be resized"),
                            &resizes,
                        ));
                    }
                    (heading, body, tr("Erase and Install"))
                }
            };

            let dialog = adw::AlertDialog::builder()
                .heading(heading)
                .body(body)
                .build();
            dialog.add_response("cancel", &tr("Cancel"));
            dialog.add_response("install", &install_label);
            if !is_fake {
                dialog.set_response_appearance("install", adw::ResponseAppearance::Destructive);
            }
            dialog.set_default_response(Some("cancel"));
            dialog.set_close_response("cancel");

            let nav_view = nav_view.clone();
            let progress_page = progress_page.clone();
            let backends = backends.clone();
            let runtime = runtime.clone();
            let button = button.clone();
            glib::spawn_future_local(async move {
                let response = dialog.choose_future(Some(&button)).await;
                if response != "install" {
                    return;
                }
                nav_view.push(&progress_page.page());
                progress_page.start(backends, runtime, cfg);
            });
        });
    }

    /// Re-fetches the target disk's live state, recomputes `plan::plan`, and
    /// repopulates every row from `cfg`. Called every time the wizard
    /// reaches this page — never assumes the disk hasn't changed since step
    /// 7 was last visited.
    pub fn refresh(&self, cfg: &InstallConfig) {
        self.language_row.set_subtitle(&cfg.language);
        self.timezone_row.set_subtitle(&cfg.timezone);
        let keyboard = if cfg.keyboard_variant.is_empty() {
            cfg.keyboard_layout.clone()
        } else {
            format!("{} ({})", cfg.keyboard_layout, cfg.keyboard_variant)
        };
        self.keyboard_row.set_subtitle(&keyboard);
        self.network_row.set_subtitle(&if cfg.network.connected {
            cfg.network
                .connection_name
                .clone()
                .unwrap_or_else(|| tr("Connected"))
        } else {
            tr("Not connected")
        });
        self.user_row.set_subtitle(&cfg.user.username);
        let de = cfg.desktop_environment;
        self.desktop_row.set_subtitle(&format!(
            "{} ({})",
            tr(crate::steps::desktop_environment::display_label(de)),
            de.de_name()
        ));
        self.apps_row.set_subtitle(&apps_label(&cfg.apps));
        self.mode_row
            .set_subtitle(&mode_label(cfg.partitioning.mode));
        self.swap_row
            .set_subtitle(&swap_label(cfg.partitioning.swap_mode));
        self.encryption_row
            .set_subtitle(&if cfg.partitioning.encryption_enabled {
                if cfg.partitioning.tpm2_enabled {
                    tr("LUKS2, unlocked via TPM2")
                } else {
                    tr("LUKS2, passphrase required at boot")
                }
            } else {
                tr("Disabled")
            });

        *self.pending_config.borrow_mut() = Some(cfg.clone());
        self.install_button.set_sensitive(false);
        self.plan_banner.set_revealed(false);
        self.erases_group.set_visible(false);
        self.resizes_group.set_visible(false);

        let Some(disk_path) = cfg.partitioning.target_disk.clone() else {
            self.disk_row.set_subtitle(&tr("No disk selected"));
            self.plan_banner.set_title(&PlanError::NoDisk.msgid());
            self.plan_banner.set_revealed(true);
            return;
        };
        self.disk_row.set_subtitle(&disk_path);
        self.spinner.start();

        let disk_backend = self.backends.disk.clone();
        let cfg_partitioning = cfg.partitioning.clone();
        let disk_row = self.disk_row.clone();
        let install_button = self.install_button.clone();
        let spinner = self.spinner.clone();
        let plan_banner = self.plan_banner.clone();
        let erases_group = self.erases_group.clone();
        let resizes_group = self.resizes_group.clone();
        let pending_erases = self.pending_erases.clone();
        let pending_resizes = self.pending_resizes.clone();
        let pending_disk_label = self.pending_disk_label.clone();
        let erase_rows = self.erase_rows.clone();
        let resize_rows = self.resize_rows.clone();

        bridge::spawn(
            &self.runtime,
            async move {
                gather_plan_input(&disk_backend, &disk_path, cfg_partitioning, detect_uefi()).await
            },
            move |result| {
                spinner.stop();
                fill_change_group(&erases_group, &erase_rows, &[]);
                fill_change_group(&resizes_group, &resize_rows, &[]);
                let input = match result {
                    Ok(input) => input,
                    Err(e) => {
                        plan_banner.set_title(&render_backend_error(&e));
                        plan_banner.set_revealed(true);
                        return;
                    }
                };
                let disk_label = format!(
                    "{} ({})",
                    input.disk.model,
                    human_bytes(input.disk.size_bytes)
                );
                disk_row.set_subtitle(&disk_label);
                *pending_disk_label.borrow_mut() = disk_label;

                match plan::plan(&input) {
                    Ok(result) => {
                        let (erases, resizes) = disk_changes(&input, &result);
                        trace_changes(&input, &erases, &resizes);
                        fill_change_group(&erases_group, &erase_rows, &erases);
                        fill_change_group(&resizes_group, &resize_rows, &resizes);
                        *pending_erases.borrow_mut() = erases;
                        *pending_resizes.borrow_mut() = resizes;
                        install_button.set_sensitive(true);
                    }
                    Err(e) => {
                        plan_banner.set_title(&e.msgid());
                        plan_banner.set_revealed(true);
                    }
                }
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::disk::layout;
    use crate::backend::disk::scenario::DiskScenario;
    use crate::config::PartitioningConfig;

    /// Assembles the `PlanInput` the review page would compute for `scenario`,
    /// the way `live_input::gather_plan_input` does but without a backend.
    ///
    /// * `scenario` - fixture whose first disk is the target.
    /// * `mode` - partitioning mode under test.
    ///
    /// # Returns
    /// The input, with the fixture's NTFS probe attached and
    /// `shrink_to_bytes` set to that probe's minimum, as the partitioning step
    /// seeds it.
    fn input_for(scenario: DiskScenario, mode: PartitionMode) -> PlanInput {
        let fixture = scenario.fixture();
        let disk = fixture.disks[0].clone();
        let partitions: Vec<_> = fixture
            .partitions
            .iter()
            .filter(|p| p.disk_path == disk.path)
            .cloned()
            .collect();
        let ntfs = partitions
            .iter()
            .find(|p| p.fs_type.as_deref() == Some("ntfs"))
            .and_then(|p| {
                fixture
                    .ntfs
                    .get(&p.path)
                    .map(|probe| (p.path.clone(), probe.clone()))
            });
        let gaps = layout::free_gaps(disk.size_bytes, &partitions);
        PlanInput {
            cfg: PartitioningConfig {
                mode,
                target_disk: Some(disk.path.clone()),
                shrink_to_bytes: ntfs.as_ref().map(|(_, probe)| probe.min_size_bytes),
                ..Default::default()
            },
            disk,
            partitions,
            gaps,
            ntfs,
            ram_bytes: 8 * 1024 * 1024 * 1024,
            uefi: true,
        }
    }

    #[test]
    fn entire_disk_lists_every_partition_in_disk_order() {
        let input = input_for(DiskScenario::Windows, PartitionMode::EntireDisk);
        let result = plan::plan(&input).unwrap();
        let (erases, resizes) = disk_changes(&input, &result);

        let names: Vec<&str> = erases.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["fake0p1", "fake0p2", "fake0p3", "fake0p4"]);
        // The MSR partition carries no filesystem — it still gets a legible
        // row rather than being dropped out of the middle of the list.
        assert!(erases[1].detail.contains("MSR"));
        assert!(erases[1].detail.contains(&tr("No filesystem")));
        assert!(resizes.is_empty());
    }

    /// Same list, but fetched the way the page really fetches it — through
    /// `--fake-disks=windows`' own backend and `gather_plan_input` — so a
    /// divergence between the fixture and what `FakeDiskBackend` serves
    /// (it renumbers every disk and partition into a `/dev/fakeN`
    /// namespace) cannot pass unnoticed.
    #[tokio::test]
    async fn entire_disk_lists_every_partition_through_the_fake_backend() {
        let disk_backend: std::sync::Arc<dyn crate::backend::disk::DiskBackend> =
            std::sync::Arc::new(crate::debugsim::FakeDiskBackend::new(
                &[DiskScenario::Windows],
                std::sync::Arc::new(crate::debugsim::FaultPlan { fault: None }),
            ));
        let cfg = PartitioningConfig {
            mode: PartitionMode::EntireDisk,
            target_disk: Some("/dev/fake0".to_string()),
            ..Default::default()
        };
        let input = gather_plan_input(&disk_backend, "/dev/fake0", cfg, true)
            .await
            .unwrap();
        let result = plan::plan(&input).unwrap();
        let (erases, resizes) = disk_changes(&input, &result);

        let names: Vec<&str> = erases.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["fake0p1", "fake0p2", "fake0p3", "fake0p4"]);
        assert!(resizes.is_empty());
    }

    #[test]
    fn alongside_windows_lists_the_shrunk_partition_only() {
        let input = input_for(DiskScenario::Windows, PartitionMode::AlongsideWindows);
        let result = plan::plan(&input).unwrap();
        let (erases, resizes) = disk_changes(&input, &result);

        assert!(erases.is_empty());
        assert_eq!(resizes.len(), 1);
        assert_eq!(resizes[0].name, "fake0p3");
        assert!(resizes[0].detail.contains("NTFS"));
        assert!(resizes[0].detail.contains('→'));
    }
}
