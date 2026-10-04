//! Final review screen — recaps every answer, recomputes `engine::plan::plan`
//! against the *live* disk state one last time (never trusts a stale plan
//! from whenever step 7 was last visited), and gates the destructive
//! "Install" action behind a confirmation dialog that explicitly lists what
//! `plan.erases` is about to destroy.

use crate::backend::Backends;
use crate::config::{AppPack, InstallConfig, PartitionMode, SwapMode};
use crate::engine::live_input::{detect_uefi, gather_plan_input};
use crate::engine::plan::{self, PlanError};
use crate::finish::progress::ProgressPage;
use crate::i18n::tr;
use crate::widgets::disk_bar::human_bytes;
use crate::{bridge, mx};
use adw::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;

fn mode_label(mode: PartitionMode) -> String {
    match mode {
        PartitionMode::EntireDisk => tr("Erase entire disk"),
        PartitionMode::AlongsideWindows => tr("Install alongside Windows"),
        PartitionMode::FreeSpace => tr("Use free space"),
        PartitionMode::Manual => tr("Manual"),
    }
}

fn pack_label(pack: AppPack) -> String {
    match pack {
        AppPack::None => tr("No applications"),
        AppPack::Base => tr("Base pack"),
    }
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
    plan_banner: adw::Banner,
    install_button: gtk::Button,
    spinner: gtk::Spinner,

    backends: Backends,
    runtime: tokio::runtime::Handle,
    nav_view: adw::NavigationView,
    progress_page: ProgressPage,
    pending_erases: Rc<RefCell<Vec<String>>>,
    pending_disk_label: Rc<RefCell<String>>,
    pending_config: Rc<RefCell<Option<InstallConfig>>>,
    erase_rows: Rc<RefCell<Vec<adw::ActionRow>>>,
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
        let apps_row = adw::ActionRow::builder().title(tr("Applications")).build();
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

        let spinner = gtk::Spinner::new();
        let install_button = gtk::Button::builder()
            .label(tr("Install"))
            .css_classes(["suggested-action", "pill"])
            .sensitive(false)
            .build();
        let button_box = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        button_box.set_halign(gtk::Align::End);
        button_box.append(&spinner);
        button_box.append(&install_button);
        let install_group = adw::PreferencesGroup::new();
        install_group.add(&button_box);

        let content = adw::PreferencesPage::new();
        content.add(&overview_group);
        content.add(&disk_group);
        content.add(&erases_group);
        content.add(&install_group);

        let page_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
        page_box.append(&plan_banner);
        page_box.append(&content);
        page_box.set_margin_bottom(12);
        page_box.set_margin_start(12);
        page_box.set_margin_end(12);

        // No `HeaderBar` of its own: `app.rs` already wraps the whole
        // `NavigationView` in one, and a second bar only stacks a duplicate row
        // of window controls under the first.
        let page = adw::NavigationPage::new(&page_box, &tr("Ready to install"));

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
            plan_banner,
            install_button,
            spinner,
            backends,
            runtime,
            nav_view,
            progress_page,
            pending_erases: Rc::new(RefCell::new(Vec::new())),
            pending_disk_label: Rc::new(RefCell::new(String::new())),
            pending_config: Rc::new(RefCell::new(None)),
            erase_rows: Rc::new(RefCell::new(Vec::new())),
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
        self.language_row.set_title(&tr("Language"));
        self.timezone_row.set_title(&tr("Timezone"));
        self.keyboard_row.set_title(&tr("Keyboard"));
        self.network_row.set_title(&tr("Network"));
        self.user_row.set_title(&tr("User account"));
        self.desktop_row.set_title(&tr("Desktop environment"));
        self.apps_row.set_title(&tr("Applications"));
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
        let pending_disk_label = self.pending_disk_label.clone();
        let pending_config = self.pending_config.clone();

        self.install_button.connect_clicked(move |button| {
            let Some(cfg) = pending_config.borrow().clone() else {
                return;
            };
            let erases = pending_erases.borrow().clone();
            let disk_label = pending_disk_label.borrow().clone();

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
                body.push_str(&format!(
                    "\n\n{}:\n{}",
                    tr("The following partitions will be permanently erased"),
                    erases
                        .iter()
                        .map(|p| format!("• {p}"))
                        .collect::<Vec<_>>()
                        .join("\n")
                ));
            }

            let dialog = adw::AlertDialog::builder()
                .heading(heading)
                .body(body)
                .build();
            dialog.add_response("cancel", &tr("Cancel"));
            dialog.add_response("install", &tr("Erase and Install"));
            dialog.set_response_appearance("install", adw::ResponseAppearance::Destructive);
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
        self.apps_row.set_subtitle(&pack_label(cfg.app_pack));
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
        let pending_erases = self.pending_erases.clone();
        let pending_disk_label = self.pending_disk_label.clone();
        let erase_rows = self.erase_rows.clone();

        bridge::spawn(
            &self.runtime,
            async move {
                gather_plan_input(&disk_backend, &disk_path, cfg_partitioning, detect_uefi()).await
            },
            move |result| {
                spinner.stop();
                for row in erase_rows.borrow_mut().drain(..) {
                    erases_group.remove(&row);
                }
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
                        *pending_erases.borrow_mut() = result.erases.clone();
                        if result.erases.is_empty() {
                            erases_group.set_visible(false);
                        } else {
                            for path in &result.erases {
                                let row = adw::ActionRow::builder().title(path.clone()).build();
                                erases_group.add(&row);
                                erase_rows.borrow_mut().push(row);
                            }
                            erases_group.set_visible(true);
                        }
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
