//! Manual partitioning editor — one ordered row per existing partition, free
//! space region, and pending creation on the target disk, laid out
//! left-to-right in disk order (matching the `DiskBar` above it). Existing
//! partitions are only ever *assigned* to a mount point here (resizing or
//! deleting one is still delegated to GParted, see `steps/partitioning.rs`'s
//! manual page); creating a new partition inside a free-space gap happens
//! in-app via the "+" button and an `adw::AlertDialog`. Every rule (exactly
//! one root, an ESP, swap size for hibernation, forced formatting…) lives in
//! `engine::plan::plan`, so this widget needs no GTK to test.

use crate::backend::disk::layout::{self, FreeGap};
use crate::backend::disk::{FormatFs, PartitionInfo, short_device_name};
use crate::engine::plan::{self, ManualEntry, ManualItem, ManualMountPoint, ManualNew};
use crate::i18n::tr;
use crate::widgets::disk_bar::human_bytes;
use crate::widgets::dropdown::size_dropdown_to_widest;
use adw::prelude::*;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

const MIB: u64 = 1024 * 1024;

/// Index 0 is always "don't use" — only the *existing*-partition row's
/// dropdown offers it; the creation dialog's mount-point picker doesn't
/// (there's no point carving a new partition to leave unused).
fn mount_point_choices() -> gtk::StringList {
    gtk::StringList::new(&[
        &tr("Don't use"),
        &tr("Root (/)"),
        &tr("Boot (/boot, EFI)"),
        &tr("Home (/home)"),
        &tr("Swap"),
    ])
}

fn choice_to_mount_point(index: u32) -> Option<ManualMountPoint> {
    match index {
        1 => Some(ManualMountPoint::Root),
        2 => Some(ManualMountPoint::Boot),
        3 => Some(ManualMountPoint::Home),
        4 => Some(ManualMountPoint::Swap),
        _ => None,
    }
}

fn mount_point_to_choice(mp: Option<ManualMountPoint>) -> u32 {
    match mp {
        None => 0,
        Some(ManualMountPoint::Root) => 1,
        Some(ManualMountPoint::Boot) => 2,
        Some(ManualMountPoint::Home) => 3,
        Some(ManualMountPoint::Swap) => 4,
    }
}

fn mount_point_to_dialog_index(mp: ManualMountPoint) -> u32 {
    match mp {
        ManualMountPoint::Root => 0,
        ManualMountPoint::Boot => 1,
        ManualMountPoint::Home => 2,
        ManualMountPoint::Swap => 3,
    }
}

fn dialog_index_to_mount_point(index: u32) -> ManualMountPoint {
    match index {
        1 => ManualMountPoint::Boot,
        2 => ManualMountPoint::Home,
        3 => ManualMountPoint::Swap,
        _ => ManualMountPoint::Root,
    }
}

fn mount_point_label(mp: ManualMountPoint) -> String {
    match mp {
        ManualMountPoint::Root => tr("Root (/)"),
        ManualMountPoint::Boot => tr("Boot (/boot, EFI)"),
        ManualMountPoint::Home => tr("Home (/home)"),
        ManualMountPoint::Swap => tr("Swap"),
    }
}

fn fs_display_name(fs: FormatFs) -> &'static str {
    match fs {
        FormatFs::Ext4 => "ext4",
        FormatFs::Btrfs => "btrfs",
        FormatFs::Xfs => "xfs",
        FormatFs::Fat32 => "FAT32",
        FormatFs::LinuxSwap => "swap",
    }
}

fn fs_string_list_for(mp: ManualMountPoint) -> gtk::StringList {
    let names: Vec<String> = plan::fs_choices(mp)
        .iter()
        .map(|f| fs_display_name(*f).to_string())
        .collect();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    gtk::StringList::new(&refs)
}

fn fs_choice_index(mp: ManualMountPoint, fs: FormatFs) -> u32 {
    plan::fs_choices(mp)
        .iter()
        .position(|f| *f == fs)
        .unwrap_or(0) as u32
}

/// The size a pending creation actually takes in its gap — mirrors
/// `engine::plan::plan_manual`'s own sizing exactly, so the "remaining free
/// space" this widget shows never drifts from what `plan()` will compute.
fn effective_size(item: &ManualNew) -> u64 {
    if item.mount_point == ManualMountPoint::Boot {
        plan::NEW_BOOT_BYTES
    } else {
        layout::align_up(item.size_bytes)
    }
}

/// ⚠ tooltip for an existing partition assigned to `mp` — `None` when
/// nothing's worth flagging. Both cases are about a formatting decision the
/// planner forces silently (`engine::plan::format_is_forced`) that the user
/// should still be told about explicitly.
fn existing_warning(part: &PartitionInfo, mp: ManualMountPoint, format: bool) -> Option<String> {
    match mp {
        ManualMountPoint::Home if !format => Some(tr(
            "Existing data in /home is kept — make sure it already holds a Linux filesystem.",
        )),
        ManualMountPoint::Boot if part.is_esp => Some(tr(
            "This partition will be reformatted as FAT32. Modulix uses its own 1024 MiB EFI partition — if this is Windows' EFI partition, Windows will no longer boot. Create a separate partition instead.",
        )),
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct ExistingAssignment {
    mount_point: Option<ManualMountPoint>,
    format: bool,
    fs: FormatFs,
}

impl Default for ExistingAssignment {
    fn default() -> Self {
        Self {
            mount_point: None,
            format: false,
            fs: FormatFs::Ext4,
        }
    }
}

enum RowData {
    Existing(PartitionInfo),
    Pending { index: usize, item: ManualNew },
    Free { gap_id: String, remaining: u64 },
}

#[derive(Clone)]
pub struct PartitionEditor {
    widget: gtk::Widget,
    list: gtk::ListBox,
    existing: Rc<RefCell<HashMap<String, ExistingAssignment>>>,
    pending: Rc<RefCell<Vec<ManualNew>>>,
    last_partitions: Rc<RefCell<Vec<PartitionInfo>>>,
    last_gaps: Rc<RefCell<Vec<FreeGap>>>,
    on_changed: Rc<RefCell<Box<dyn Fn()>>>,
}

impl PartitionEditor {
    pub fn new() -> Self {
        let list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .css_classes(["boxed-list"])
            .build();
        let widget: gtk::Widget = list.clone().upcast();

        Self {
            widget,
            list,
            existing: Default::default(),
            pending: Default::default(),
            last_partitions: Default::default(),
            last_gaps: Default::default(),
            on_changed: Rc::new(RefCell::new(Box::new(|| {}))),
        }
    }

    pub fn widget(&self) -> gtk::Widget {
        self.widget.clone()
    }

    pub fn connect_changed<F: Fn() + 'static>(&self, f: F) {
        *self.on_changed.borrow_mut() = Box::new(f);
    }

    /// Rebuilds the row list from the target disk's current partitions and
    /// free-space gaps. Assignments for paths still present, and pending
    /// creations whose gap still exists with enough room, survive the
    /// rebuild — important since re-reading the disk (the refresh path,
    /// notably after GParted) must not silently drop the user's work.
    pub fn set_data(&self, partitions: &[PartitionInfo], gaps: &[FreeGap]) {
        self.existing
            .borrow_mut()
            .retain(|path, _| partitions.iter().any(|p| &p.path == path));
        self.pending.borrow_mut().retain(|item| {
            gaps.iter()
                .find(|g| g.id == item.gap_id)
                .is_some_and(|g| effective_size(item) <= g.size_bytes)
        });

        *self.last_partitions.borrow_mut() = partitions.to_vec();
        *self.last_gaps.borrow_mut() = gaps.to_vec();

        self.rebuild();
    }

    /// Rows in disk order: existing partitions, plus per-gap `Pending`
    /// creations stacked from the gap's start (in the order they appear in
    /// `self.pending`, same as `engine::plan::plan_manual`'s own cursor
    /// logic) followed by whatever's left as a `Free` row.
    fn layout_rows(&self) -> Vec<(u64, RowData)> {
        let mut rows: Vec<(u64, RowData)> = self
            .last_partitions
            .borrow()
            .iter()
            .map(|p| (p.start_bytes, RowData::Existing(p.clone())))
            .collect();

        let pending = self.pending.borrow();
        for gap in self.last_gaps.borrow().iter() {
            let mut cursor = gap.start_bytes;
            for (index, item) in pending.iter().enumerate() {
                if item.gap_id != gap.id {
                    continue;
                }
                rows.push((
                    cursor,
                    RowData::Pending {
                        index,
                        item: item.clone(),
                    },
                ));
                cursor += effective_size(item);
            }
            let remaining = (gap.start_bytes + gap.size_bytes).saturating_sub(cursor);
            if remaining > 0 {
                rows.push((
                    cursor,
                    RowData::Free {
                        gap_id: gap.id.clone(),
                        remaining,
                    },
                ));
            }
        }

        rows.sort_by_key(|(start, _)| *start);
        rows
    }

    /// Bytes still free in gap `gap_id` if the pending creation at
    /// `exclude_index` (the one being edited, if any) didn't count against
    /// it — the upper bound the creation dialog offers.
    fn gap_capacity(&self, gap_id: &str, exclude_index: Option<usize>) -> u64 {
        let Some(gap) = self
            .last_gaps
            .borrow()
            .iter()
            .find(|g| g.id == gap_id)
            .cloned()
        else {
            return 0;
        };
        let used: u64 = self
            .pending
            .borrow()
            .iter()
            .enumerate()
            .filter(|(i, item)| item.gap_id == gap_id && Some(*i) != exclude_index)
            .map(|(_, item)| effective_size(item))
            .sum();
        gap.size_bytes.saturating_sub(used)
    }

    fn rebuild(&self) {
        while let Some(child) = self.list.first_child() {
            self.list.remove(&child);
        }
        for (_, data) in self.layout_rows() {
            match data {
                RowData::Existing(part) => self.build_existing_row(&part),
                RowData::Pending { index, item } => self.build_pending_row(index, &item),
                RowData::Free { gap_id, remaining } => self.build_free_row(&gap_id, remaining),
            }
        }
    }

    fn notify_and_rebuild(&self) {
        self.rebuild();
        (self.on_changed.borrow())();
    }

    fn build_existing_row(&self, part: &PartitionInfo) {
        let assignment = self
            .existing
            .borrow()
            .get(&part.path)
            .copied()
            .unwrap_or_default();
        let fs_mp = assignment.mount_point.unwrap_or(ManualMountPoint::Root);

        let choice = gtk::DropDown::builder()
            .model(&mount_point_choices())
            .selected(mount_point_to_choice(assignment.mount_point))
            .build();
        size_dropdown_to_widest(&choice, &mount_point_choices());
        let format = gtk::CheckButton::builder()
            .label(tr("Format"))
            .active(assignment.format)
            .build();
        format.set_visible(assignment.mount_point == Some(ManualMountPoint::Home));
        let fs = gtk::DropDown::builder()
            .model(&fs_string_list_for(fs_mp))
            .selected(fs_choice_index(fs_mp, assignment.fs))
            .build();
        size_dropdown_to_widest(&fs, &fs_string_list_for(fs_mp));
        fs.set_visible(matches!(
            assignment.mount_point,
            Some(ManualMountPoint::Root) | Some(ManualMountPoint::Home)
        ));

        let warning = gtk::Image::from_icon_name("dialog-warning-symbolic");
        warning.set_visible(false);
        if let Some(mp) = assignment.mount_point
            && let Some(message) = existing_warning(part, mp, assignment.format)
        {
            warning.set_tooltip_text(Some(&message));
            warning.set_visible(true);
        }

        let title = format!(
            "{} — {} ({})",
            part.label
                .clone()
                .unwrap_or_else(|| short_device_name(&part.path).to_string()),
            part.fs_type.clone().unwrap_or_else(|| tr("unformatted")),
            human_bytes(part.size_bytes)
        );
        let row = adw::ActionRow::builder().title(title).build();
        row.add_suffix(&warning);
        row.add_suffix(&choice);
        row.add_suffix(&format);
        row.add_suffix(&fs);
        self.list.append(&row);

        {
            let this = self.clone();
            let path = part.path.clone();
            choice.connect_selected_notify(move |dd| {
                this.existing
                    .borrow_mut()
                    .entry(path.clone())
                    .or_default()
                    .mount_point = choice_to_mount_point(dd.selected());
                this.notify_and_rebuild();
            });
        }
        {
            let this = self.clone();
            let path = part.path.clone();
            format.connect_toggled(move |cb| {
                this.existing
                    .borrow_mut()
                    .entry(path.clone())
                    .or_default()
                    .format = cb.is_active();
                this.notify_and_rebuild();
            });
        }
        {
            let this = self.clone();
            let path = part.path.clone();
            fs.connect_selected_notify(move |dd| {
                let mut existing = this.existing.borrow_mut();
                let entry = existing.entry(path.clone()).or_default();
                let mp = entry.mount_point.unwrap_or(ManualMountPoint::Root);
                if let Some(picked) = plan::fs_choices(mp).get(dd.selected() as usize) {
                    entry.fs = *picked;
                }
                drop(existing);
                this.notify_and_rebuild();
            });
        }
    }

    fn build_pending_row(&self, index: usize, item: &ManualNew) {
        let title = format!(
            "{} — {}",
            tr("New partition"),
            human_bytes(effective_size(item))
        );
        let subtitle = format!(
            "{} · {}",
            mount_point_label(item.mount_point),
            fs_display_name(item.fs)
        );
        let row = adw::ActionRow::builder()
            .title(title)
            .subtitle(subtitle)
            .activatable(true)
            .build();

        let trash = gtk::Button::from_icon_name("user-trash-symbolic");
        trash.add_css_class("flat");
        trash.set_valign(gtk::Align::Center);
        trash.set_tooltip_text(Some(&tr("Remove")));
        row.add_suffix(&trash);

        {
            let this = self.clone();
            trash.connect_clicked(move |_| {
                if index < this.pending.borrow().len() {
                    this.pending.borrow_mut().remove(index);
                }
                this.notify_and_rebuild();
            });
        }
        {
            let this = self.clone();
            let gap_id = item.gap_id.clone();
            row.connect_activated(move |_| {
                this.open_create_dialog(gap_id.clone(), Some(index));
            });
        }

        self.list.append(&row);
    }

    fn build_free_row(&self, gap_id: &str, remaining: u64) {
        let title = format!("{} — {}", tr("Free space"), human_bytes(remaining));
        let row = adw::ActionRow::builder().title(title).build();

        let add = gtk::Button::from_icon_name("list-add-symbolic");
        add.add_css_class("flat");
        add.set_valign(gtk::Align::Center);
        add.set_sensitive(remaining >= layout::ALIGNMENT);
        add.set_tooltip_text(Some(&tr("Create a partition here")));
        row.add_suffix(&add);

        {
            let this = self.clone();
            let gap_id = gap_id.to_string();
            add.connect_clicked(move |_| {
                this.open_create_dialog(gap_id.clone(), None);
            });
        }

        self.list.append(&row);
    }

    /// Opens the creation/edit dialog for gap `gap_id` — `prefill_index`
    /// (`Some` when reopened from a `Pending` row) makes it an edit of that
    /// creation instead of a fresh one, prefilled with its current values
    /// and excluded from its own capacity computation.
    fn open_create_dialog(&self, gap_id: String, prefill_index: Option<usize>) {
        let capacity = self.gap_capacity(&gap_id, prefill_index);
        let prefill = prefill_index.and_then(|idx| self.pending.borrow().get(idx).cloned());

        let start_mp = prefill
            .as_ref()
            .map(|it| it.mount_point)
            .unwrap_or(ManualMountPoint::Root);
        let start_fs = prefill
            .as_ref()
            .map(|it| it.fs)
            .unwrap_or_else(|| plan::fs_choices(start_mp)[0]);
        let start_size = prefill.as_ref().map(|it| it.size_bytes).unwrap_or(capacity);

        let group = adw::PreferencesGroup::new();

        let mp_choices = gtk::StringList::new(&[
            &mount_point_label(ManualMountPoint::Root),
            &mount_point_label(ManualMountPoint::Boot),
            &mount_point_label(ManualMountPoint::Home),
            &mount_point_label(ManualMountPoint::Swap),
        ]);
        let mp_row = adw::ComboRow::builder()
            .title(tr("Mount point"))
            .model(&mp_choices)
            .build();
        size_dropdown_to_widest(&mp_row, &mp_choices);
        mp_row.set_selected(mount_point_to_dialog_index(start_mp));
        group.add(&mp_row);

        let fs_row = adw::ComboRow::builder().title(tr("Filesystem")).build();
        // Sized off every possible filesystem, not just the current mount
        // point's subset — `apply_mount_point` below swaps `fs_row`'s model
        // as the user changes the mount point, and re-measuring only the
        // active subset each time would make the row visibly resize.
        let fs_all_names = gtk::StringList::new(&[
            fs_display_name(FormatFs::Ext4),
            fs_display_name(FormatFs::Btrfs),
            fs_display_name(FormatFs::Xfs),
            fs_display_name(FormatFs::Fat32),
            fs_display_name(FormatFs::LinuxSwap),
        ]);
        size_dropdown_to_widest(&fs_row, &fs_all_names);
        group.add(&fs_row);

        let start_mib = (start_size / MIB).max(1) as f64;
        let size_adj = gtk::Adjustment::new(start_mib, 1.0, start_mib, 1.0, 16.0, 0.0);
        let size_row = adw::SpinRow::builder()
            .title(tr("Size (MiB)"))
            .adjustment(&size_adj)
            .digits(0)
            .build();
        group.add(&size_row);

        let dialog = adw::AlertDialog::builder()
            .heading(if prefill_index.is_some() {
                tr("Edit partition")
            } else {
                tr("Create partition")
            })
            .extra_child(&group)
            .build();
        dialog.add_response("cancel", &tr("Cancel"));
        dialog.add_response(
            "add",
            &if prefill_index.is_some() {
                tr("Save")
            } else {
                tr("Add")
            },
        );
        dialog.set_response_appearance("add", adw::ResponseAppearance::Suggested);
        dialog.set_default_response(Some("add"));
        dialog.set_close_response("cancel");

        let apply_mount_point: Rc<dyn Fn(ManualMountPoint)> = {
            let fs_row = fs_row.clone();
            let size_row = size_row.clone();
            let dialog = dialog.clone();
            Rc::new(move |mp: ManualMountPoint| {
                let choices = plan::fs_choices(mp);
                let names: Vec<String> = choices
                    .iter()
                    .map(|f| fs_display_name(*f).to_string())
                    .collect();
                let refs: Vec<&str> = names.iter().map(String::as_str).collect();
                fs_row.set_model(Some(&gtk::StringList::new(&refs)));
                fs_row.set_selected(0);
                fs_row.set_visible(choices.len() > 1);

                let lower = (plan::min_size_bytes(mp) / MIB).max(1);
                let upper = (capacity / MIB).max(lower);
                let adj = size_row.adjustment();
                adj.set_lower(lower as f64);
                adj.set_upper(upper as f64);
                if mp == ManualMountPoint::Boot {
                    size_row.set_value(lower as f64);
                    size_row.set_sensitive(false);
                } else {
                    size_row.set_sensitive(true);
                    if size_row.value() < lower as f64 || size_row.value() > upper as f64 {
                        size_row.set_value(upper as f64);
                    }
                }

                dialog.set_response_enabled("add", capacity >= plan::min_size_bytes(mp));
            })
        };
        apply_mount_point(start_mp);
        if let Some(idx) = plan::fs_choices(start_mp)
            .iter()
            .position(|f| *f == start_fs)
        {
            fs_row.set_selected(idx as u32);
        }
        size_row.set_value(start_mib);

        {
            let apply_mount_point = apply_mount_point.clone();
            mp_row.connect_selected_notify(move |row| {
                apply_mount_point(dialog_index_to_mount_point(row.selected()));
            });
        }

        let this = self.clone();
        let list = self.list.clone();
        let mp_row = mp_row.clone();
        let fs_row = fs_row.clone();
        let size_row = size_row.clone();
        glib::spawn_future_local(async move {
            let response = dialog.choose_future(Some(&list)).await;
            if response != "add" {
                return;
            }
            let mp = dialog_index_to_mount_point(mp_row.selected());
            let choices = plan::fs_choices(mp);
            let fs = choices
                .get(fs_row.selected() as usize)
                .copied()
                .unwrap_or(choices[0]);
            let size_bytes = if mp == ManualMountPoint::Boot {
                plan::NEW_BOOT_BYTES
            } else {
                (size_row.value() as u64) * MIB
            };
            let new_item = ManualNew {
                gap_id,
                size_bytes,
                mount_point: mp,
                fs,
            };
            {
                let mut pending = this.pending.borrow_mut();
                match prefill_index {
                    Some(idx) if idx < pending.len() => pending[idx] = new_item,
                    _ => pending.push(new_item),
                }
            }
            this.notify_and_rebuild();
        });
    }

    /// `entries()` in row order — existing partitions still get to keep
    /// their disk position even though `ManualItem` doesn't carry one
    /// explicitly; `plan_manual` only needs `New` items grouped and ordered
    /// by `gap_id`, which this preserves since `Pending` rows are emitted in
    /// `self.pending`'s own order within each gap (see `layout_rows`).
    pub fn entries(&self) -> Vec<ManualItem> {
        self.layout_rows()
            .into_iter()
            .filter_map(|(_, data)| match data {
                RowData::Existing(part) => {
                    let assignment = self.existing.borrow().get(&part.path).copied()?;
                    let mp = assignment.mount_point?;
                    Some(ManualItem::Existing(ManualEntry {
                        path: part.path.clone(),
                        mount_point: mp,
                        format: assignment.format,
                        fs: assignment.fs,
                    }))
                }
                RowData::Pending { item, .. } => Some(ManualItem::New(item)),
                RowData::Free { .. } => None,
            })
            .collect()
    }

    /// Rebuilds every row's title/dropdown labels with fresh `tr()` text
    /// after a language change — `set_data()` already preserves
    /// assignments/pending creations, so calling it again with the last-seen
    /// disk state has no other side effect.
    pub fn retranslate(&self) {
        let partitions = self.last_partitions.borrow().clone();
        let gaps = self.last_gaps.borrow().clone();
        self.set_data(&partitions, &gaps);
    }
}

impl Default for PartitionEditor {
    fn default() -> Self {
        Self::new()
    }
}
