use crate::backend::Backends;
use crate::bridge;
use crate::config::{InstallConfig, PartitionMode, SwapMode};
use crate::i18n::tr;
use crate::mx;
use crate::steps::{Step, StepId, ValidityTracker};
use crate::widgets::DiskBar;
use adw::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;

const MODES: [PartitionMode; 4] = [
    PartitionMode::EntireDisk,
    PartitionMode::AlongsideWindows,
    PartitionMode::FreeSpace,
    PartitionMode::Manual,
];
const SWAP_MODES: [SwapMode; 3] = [SwapMode::None, SwapMode::Standard, SwapMode::Hibernation];

fn mode_label(mode: PartitionMode) -> &'static str {
    match mode {
        PartitionMode::EntireDisk => "Erase entire disk",
        PartitionMode::AlongsideWindows => "Install alongside Windows",
        PartitionMode::FreeSpace => "Use free space",
        PartitionMode::Manual => "Manual",
    }
}

fn swap_label(mode: SwapMode) -> &'static str {
    match mode {
        SwapMode::None => "No swap",
        SwapMode::Standard => "Standard swap",
        SwapMode::Hibernation => "Swap with hibernation support",
    }
}

fn mode_string_list() -> gtk::StringList {
    let names: Vec<String> = MODES.iter().map(|m| tr(mode_label(*m))).collect();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    gtk::StringList::new(&refs)
}

fn swap_string_list() -> gtk::StringList {
    let names: Vec<String> = SWAP_MODES.iter().map(|m| tr(swap_label(*m))).collect();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    gtk::StringList::new(&refs)
}

/// Step 7 — stub for iteration 1: disk/mode/swap/encryption selection with
/// `commit()` wired up, but the actual partitioning plan (NTFS-shrink
/// preflight, manual editor) is iteration 2 — see CLAUDE.md's risk section.
pub struct PartitioningStep {
    widget: gtk::Widget,
    group: adw::PreferencesGroup,
    disk_row: adw::ActionRow,
    mode_row: adw::ActionRow,
    mode_dropdown: gtk::DropDown,
    swap_row: adw::ActionRow,
    swap_dropdown: gtk::DropDown,
    encryption_row: adw::SwitchRow,
    tpm2_row: adw::SwitchRow,
    selected_disk: Rc<RefCell<Option<String>>>,
    selected_mode: Rc<RefCell<PartitionMode>>,
    selected_swap: Rc<RefCell<SwapMode>>,
    encryption_enabled: Rc<RefCell<bool>>,
    tpm2_enabled: Rc<RefCell<bool>>,
    validity: ValidityTracker,
}

impl PartitioningStep {
    pub fn new(backends: &Backends, runtime: &tokio::runtime::Handle) -> Self {
        let disks: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
        let selected_disk: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
        let selected_mode = Rc::new(RefCell::new(PartitionMode::EntireDisk));
        let selected_swap = Rc::new(RefCell::new(SwapMode::Standard));
        let encryption_enabled = Rc::new(RefCell::new(false));
        let tpm2_enabled = Rc::new(RefCell::new(false));

        let disk_dropdown = gtk::DropDown::from_strings(&[]);
        let disk_row = adw::ActionRow::builder().title(tr("Target disk")).build();
        disk_row.add_suffix(&disk_dropdown);

        let mode_dropdown = gtk::DropDown::from_strings(&[]);
        mode_dropdown.set_model(Some(&mode_string_list()));
        let mode_row = adw::ActionRow::builder()
            .title(tr("Partitioning mode"))
            .build();
        mode_row.add_suffix(&mode_dropdown);

        let swap_dropdown = gtk::DropDown::from_strings(&[]);
        swap_dropdown.set_model(Some(&swap_string_list()));
        swap_dropdown.set_selected(1);
        let swap_row = adw::ActionRow::builder().title(tr("Swap")).build();
        swap_row.add_suffix(&swap_dropdown);

        let encryption_row = adw::SwitchRow::builder()
            .title(tr("Encrypt disk"))
            .subtitle(tr("LUKS2, unlocked via TPM2 when available"))
            .build();
        let tpm2_row = adw::SwitchRow::builder()
            .title(tr("Use TPM2"))
            .subtitle(tr("Unlock automatically without a passphrase"))
            .build();
        tpm2_row.set_sensitive(false);

        let group = adw::PreferencesGroup::builder()
            .title(tr("Partitioning"))
            .build();
        group.add(&disk_row);
        group.add(&mode_row);
        group.add(&swap_row);
        group.add(&encryption_row);
        group.add(&tpm2_row);

        let disk_bar = DiskBar::new();

        let container = gtk::Box::new(gtk::Orientation::Vertical, 12);
        let page = adw::PreferencesPage::new();
        page.add(&group);
        container.append(&page);
        container.append(&disk_bar.widget());

        {
            let disk_backend = backends.disk.clone();
            let disk_dropdown = disk_dropdown.clone();
            let disks = disks.clone();
            let selected_disk = selected_disk.clone();
            let disk_bar = disk_bar.clone();
            bridge::spawn(
                runtime,
                async move { disk_backend.list_disks().await },
                move |result| {
                    let Ok(loaded) = result else { return };
                    if loaded.is_empty() {
                        return;
                    }
                    let names: Vec<String> = loaded
                        .iter()
                        .map(|d| format!("{} ({} GB)", d.model, d.size_bytes / 1_000_000_000))
                        .collect();
                    let name_refs: Vec<&str> = names.iter().map(String::as_str).collect();
                    disk_dropdown.set_model(Some(&gtk::StringList::new(&name_refs)));
                    *selected_disk.borrow_mut() = Some(loaded[0].path.clone());
                    disk_bar.set_segments(vec![(loaded[0].model.clone(), loaded[0].size_bytes)]);
                    *disks.borrow_mut() = loaded.into_iter().map(|d| d.path).collect();
                },
            );
        }

        {
            let disks = disks.clone();
            let selected_disk = selected_disk.clone();
            disk_dropdown.connect_selected_notify(move |dd| {
                let idx = dd.selected() as usize;
                if let Some(path) = disks.borrow().get(idx) {
                    *selected_disk.borrow_mut() = Some(path.clone());
                }
            });
        }

        {
            let selected_mode = selected_mode.clone();
            mode_dropdown.connect_selected_notify(move |dd| {
                let idx = dd.selected() as usize;
                *selected_mode.borrow_mut() =
                    MODES.get(idx).copied().unwrap_or(PartitionMode::EntireDisk);
            });
        }

        {
            let selected_swap = selected_swap.clone();
            swap_dropdown.connect_selected_notify(move |dd| {
                let idx = dd.selected() as usize;
                *selected_swap.borrow_mut() =
                    SWAP_MODES.get(idx).copied().unwrap_or(SwapMode::None);
            });
        }

        {
            let encryption_enabled = encryption_enabled.clone();
            let tpm2_row = tpm2_row.clone();
            encryption_row.connect_active_notify(move |row| {
                let enabled = row.is_active();
                *encryption_enabled.borrow_mut() = enabled;
                tpm2_row.set_sensitive(enabled);
                if !enabled {
                    tpm2_row.set_active(false);
                }
            });
        }

        {
            let tpm2_enabled = tpm2_enabled.clone();
            tpm2_row.connect_active_notify(move |row| {
                *tpm2_enabled.borrow_mut() = row.is_active();
            });
        }

        Self {
            widget: container.upcast(),
            group,
            disk_row,
            mode_row,
            mode_dropdown,
            swap_row,
            swap_dropdown,
            encryption_row,
            tpm2_row,
            selected_disk,
            selected_mode,
            selected_swap,
            encryption_enabled,
            tpm2_enabled,
            validity: ValidityTracker::ready(),
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
        cfg.partitioning.target_disk = self.selected_disk.borrow().clone();
        cfg.partitioning.mode = *self.selected_mode.borrow();
        cfg.partitioning.swap_mode = *self.selected_swap.borrow();
        cfg.partitioning.encryption_enabled = *self.encryption_enabled.borrow();
        cfg.partitioning.tpm2_enabled = *self.tpm2_enabled.borrow();
        Ok(())
    }

    fn retranslate(&self) {
        self.group.set_title(&tr("Partitioning"));
        self.disk_row.set_title(&tr("Target disk"));
        self.mode_row.set_title(&tr("Partitioning mode"));
        let mode_selected = self.mode_dropdown.selected();
        self.mode_dropdown.set_model(Some(&mode_string_list()));
        self.mode_dropdown.set_selected(mode_selected);
        self.swap_row.set_title(&tr("Swap"));
        let swap_selected = self.swap_dropdown.selected();
        self.swap_dropdown.set_model(Some(&swap_string_list()));
        self.swap_dropdown.set_selected(swap_selected);
        self.encryption_row.set_title(&tr("Encrypt disk"));
        self.encryption_row
            .set_subtitle(&tr("LUKS2, unlocked via TPM2 when available"));
        self.tpm2_row.set_title(&tr("Use TPM2"));
        self.tpm2_row
            .set_subtitle(&tr("Unlock automatically without a passphrase"));
    }
}
