//! Install progress screen — an auto-advancing slideshow fills the main
//! area while `Task`/`Pipeline` (see `engine`) runs in the background, log
//! tucked away in a collapsed `Expander` and the progress bar pinned to the
//! bottom. Pushed by `SummaryPage` once the user confirms the
//! destructive-action dialog.

use crate::backend::Backends;
use crate::config::InstallConfig;
use crate::engine::{self, ProgressEvent, TaskCtx, install_log};
use crate::finish::qr_report;
use crate::finish::slides::INSTALL_SLIDES;
use crate::i18n::tr;
use crate::mx;
use crate::widgets::qr_code;
use crate::widgets::slideshow::Slideshow;
use adw::prelude::*;

/// Hard cap on the install log: the tail is what matters, and an
/// unbounded `TextBuffer` would grow for the whole `nixos-install`.
const LOG_MAX_LINES: i32 = 2000;

#[derive(Clone)]
pub struct ProgressPage {
    page: adw::NavigationPage,
    progress_bar: gtk::ProgressBar,
    /// Live while a task reports no fraction of its own; see
    /// `ProgressEvent::Indeterminate`.
    pulse_source: std::rc::Rc<std::cell::RefCell<Option<glib::SourceId>>>,
    status_label: gtk::Label,
    log_buffer: gtk::TextBuffer,
    log_view: gtk::TextView,
    log_expander: gtk::Expander,
    result_box: gtk::Box,
    result_label: gtk::Label,
    /// Where the full log was persisted. Hidden until the install fails,
    /// which is the only time the user needs to go read it.
    log_path_label: gtk::Label,
    /// Offered once the install succeeded: the live session has no other
    /// way out, the installer is the only app in the kiosk.
    restart_button: gtk::Button,
    /// Offered once the install failed, in the same spot: the kiosk has no
    /// browser and no way to get text out, so the report leaves by phone
    /// camera. See [`crate::finish::qr_report`].
    qr_button: gtk::Button,
    /// Also offered once the install failed: the way out of a failed install
    /// is the same reboot the success path offers, behind the same
    /// remove-the-medium warning. Destructive-looking on purpose — nothing was
    /// installed, so leaving costs the user their session.
    quit_button: gtk::Button,
    /// Error message and persisted-log paths of the last failure, as handed to
    /// [`ProgressPage::show_failure`]. Read by the QR button's handler.
    failure: std::rc::Rc<std::cell::RefCell<(String, Vec<String>)>>,
    log_scroller: gtk::ScrolledWindow,
    slideshow: std::rc::Rc<Slideshow>,
}

impl ProgressPage {
    pub fn new() -> Self {
        let slideshow = std::rc::Rc::new(Slideshow::new(INSTALL_SLIDES));

        let status_label = gtk::Label::builder()
            .xalign(0.0)
            .wrap(true)
            .css_classes(["title-4"])
            .build();
        let progress_bar = gtk::ProgressBar::builder().show_text(false).build();

        // Selectable, `WordChar`-wrapped and inside a scroller: a failed
        // `nixos-install` reports Nix traces and device paths that have no
        // space to break on, and the whole message has to stay readable
        // instead of stretching the page off-screen.
        let result_label = gtk::Label::builder()
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .selectable(true)
            .xalign(0.0)
            .hexpand(true)
            .build();
        let log_path_label = gtk::Label::builder()
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .selectable(true)
            .xalign(0.0)
            .hexpand(true)
            .visible(false)
            .css_classes(["dim-label"])
            .build();
        let result_text = gtk::Box::new(gtk::Orientation::Vertical, 6);
        result_text.append(&result_label);
        result_text.append(&log_path_label);
        let result_scroller = gtk::ScrolledWindow::builder()
            .child(&result_text)
            .hexpand(true)
            .propagate_natural_height(true)
            .max_content_height(220)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .build();
        let restart_button = gtk::Button::builder()
            .label(tr("Restart now"))
            .css_classes(["suggested-action", "pill"])
            .halign(gtk::Align::End)
            .valign(gtk::Align::Center)
            .visible(false)
            .build();

        let qr_button = gtk::Button::builder()
            .child(
                &adw::ButtonContent::builder()
                    .icon_name("qr-code-symbolic")
                    .label(tr("Error report QR code"))
                    .build(),
            )
            .css_classes(["pill"])
            .halign(gtk::Align::End)
            .valign(gtk::Align::Center)
            .visible(false)
            .build();

        let quit_button = gtk::Button::builder()
            .label(tr("Quit the installer"))
            .css_classes(["destructive-action", "pill"])
            .halign(gtk::Align::End)
            .valign(gtk::Align::Center)
            .visible(false)
            .build();

        // The outcome row takes the progress bar's place once the pipeline is
        // done — outcome text on the left, the actions on the right — rather
        // than stacking a card on top of a bar that no longer moves. Success
        // shows `restart_button`, failure `qr_button` + `quit_button`.
        let result_box = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        result_box.append(&result_scroller);
        result_box.append(&qr_button);
        result_box.append(&quit_button);
        result_box.append(&restart_button);
        result_box.set_visible(false);

        let log_view = gtk::TextView::builder()
            .editable(false)
            .monospace(true)
            .cursor_visible(false)
            .top_margin(6)
            .bottom_margin(6)
            .left_margin(6)
            .right_margin(6)
            .build();
        let log_buffer = log_view.buffer();
        let log_scroller = gtk::ScrolledWindow::builder()
            .child(&log_view)
            .vexpand(false)
            .min_content_height(180)
            .build();
        log_scroller.add_css_class("card");

        let log_expander = gtk::Expander::builder()
            .label(tr("Details"))
            .expanded(false)
            .child(&log_scroller)
            .build();

        let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
        content.set_margin_top(24);
        content.set_margin_bottom(24);
        content.set_margin_start(24);
        content.set_margin_end(24);
        content.append(&slideshow.widget());
        content.append(&log_expander);
        content.append(&status_label);
        content.append(&progress_bar);
        content.append(&result_box);

        // No `HeaderBar` of its own: `app.rs` already wraps the whole
        // `NavigationView` in one, and a second bar only stacks a duplicate row
        // of window controls under the first.
        let page = adw::NavigationPage::new(&content, &tr("Installing"));
        page.set_can_pop(false);

        let this = Self {
            page,
            progress_bar,
            pulse_source: std::rc::Rc::new(std::cell::RefCell::new(None)),
            status_label,
            log_buffer,
            log_view,
            log_expander,
            result_box,
            result_label,
            log_path_label,
            restart_button,
            qr_button,
            quit_button,
            failure: std::rc::Rc::new(std::cell::RefCell::new((String::new(), Vec::new()))),
            log_scroller,
            slideshow,
        };
        this.wire_reboot_button(&this.restart_button);
        this.wire_reboot_button(&this.quit_button);
        this.wire_qr_button();
        this
    }

    pub fn page(&self) -> adw::NavigationPage {
        self.page.clone()
    }

    /// Starts `engine::full_pipeline()` on `runtime` and streams its
    /// `ProgressEvent`s back onto the GTK main loop. The pipeline's overall
    /// `Result` travels over its own one-shot channel (`done_rx`) rather than
    /// being inferred from the log stream — cheap and unambiguous.
    pub fn start(
        &self,
        backends: Backends,
        runtime: tokio::runtime::Handle,
        config: InstallConfig,
    ) {
        self.progress_bar.set_fraction(0.0);
        self.status_label.set_label(&tr("Starting installation…"));
        self.log_buffer.set_text("");
        self.result_box.set_visible(false);
        self.restart_button.set_visible(false);
        self.restart_button.set_sensitive(true);
        self.qr_button.set_visible(false);
        self.quit_button.set_visible(false);
        self.quit_button.set_sensitive(true);
        self.quit_button.set_label(&tr("Quit the installer"));
        self.qr_button.set_sensitive(true);
        if let Some(content) = self.qr_button.child().and_downcast::<adw::ButtonContent>() {
            content.set_label(&tr("Error report QR code"));
        }
        self.log_path_label.set_visible(false);
        self.status_label.set_visible(true);
        self.progress_bar.set_visible(true);
        self.result_label.remove_css_class("title-4");
        self.log_scroller.set_min_content_height(180);
        self.page.set_title(&tr("Installing"));
        self.log_expander.set_label(Some(&tr("Details")));
        self.slideshow.retranslate();
        self.slideshow.start();
        self.set_pulsing(false);

        // Two hops, not one: the pipeline feeds `raw_tx`, a tokio task mirrors
        // every event into `install_log` and forwards it to the GTK loop over
        // `ui_tx`. Writing the file from the `spawn_future_local` consumer
        // below would put tens of thousands of `nixos-install` lines of
        // filesystem I/O on the main loop.
        let (raw_tx, raw_rx) = async_channel::unbounded::<ProgressEvent>();
        let (ui_tx, rx) = async_channel::unbounded::<ProgressEvent>();
        let (done_tx, done_rx) = async_channel::bounded::<mx::Result<()>>(1);
        let (tee_done_tx, tee_done_rx) = async_channel::bounded::<()>(1);

        runtime.spawn(async move {
            let mut writer = install_log::Writer::open().await;
            while let Ok(event) = raw_rx.recv().await {
                writer.record(&event).await;
                // A closed UI must not truncate the file.
                let _ = ui_tx.send(event).await;
            }
            writer.flush().await;
            // Failure path: the pipeline aborted before `PostInstallTask`, so
            // the target is still mounted and can take the copy.
            install_log::copy_to_target().await;
            let _ = tee_done_tx.send(()).await;
        });

        runtime.spawn(async move {
            let ctx = TaskCtx::new(backends, config);
            let result = engine::full_pipeline().run(&ctx, &raw_tx).await;
            // Untranslated on purpose: this line lands in the log file and in
            // journald, which are read by whoever debugs the install. The UI
            // gets the translated message from `done_rx`.
            let outcome = match &result {
                Ok(()) => "install finished successfully".to_string(),
                Err(e) => format!("INSTALL FAILED: {e}"),
            };
            let _ = raw_tx.send(ProgressEvent::Log(outcome)).await;
            drop(raw_tx);
            // The tee owns the log file and the target copy; wait for it so
            // the failure page can only name copies that really exist.
            let _ = tee_done_rx.recv().await;
            let _ = done_tx.send(result).await;
        });

        {
            let this = self.clone();
            glib::spawn_future_local(async move {
                while let Ok(event) = rx.recv().await {
                    match event {
                        ProgressEvent::Started { task } => this.status_label.set_label(&tr(&task)),
                        ProgressEvent::Log(line) => this.append_log(&line),
                        ProgressEvent::Progress { fraction } => {
                            this.progress_bar.set_fraction(fraction);
                        }
                        ProgressEvent::Indeterminate(on) => this.set_pulsing(on),
                        ProgressEvent::Finished { task } => {
                            this.append_log(&format!("\u{2713} {task}"));
                        }
                    }
                }
            });
        }

        {
            let this = self.clone();
            glib::spawn_future_local(async move {
                let outcome = done_rx.recv().await;
                this.slideshow.stop();
                this.set_pulsing(false);
                this.result_box.set_visible(true);
                match outcome {
                    Ok(Ok(())) => {
                        // Success needs no running commentary: the progress bar
                        // and the per-task status line give way to the outcome
                        // row they were sitting above.
                        this.status_label.set_visible(false);
                        this.progress_bar.set_visible(false);
                        this.result_label.add_css_class("title-4");
                        this.result_label.set_label(&tr("Installation complete"));
                        this.restart_button.set_visible(true);
                    }
                    Ok(Err(e)) => this.show_failure(&mx::render(&e)),
                    Err(_) => this.show_failure(&tr("The installer stopped unexpectedly")),
                }
            });
        }
    }

    /// Connects a button that reboots the machine behind the
    /// remove-the-medium confirmation. The dialog is not a formality: the
    /// firmware would boot the live ISO again and the user would believe the
    /// install failed, so the medium has to be unplugged *before* the reboot,
    /// while the running system still lives in the RAM overlay and no longer
    /// needs the USB drive.
    ///
    /// Shared by the success path's "Restart now" and the failure path's
    /// "Quit the installer": leaving a failed install means rebooting too, and
    /// the same warning applies.
    ///
    /// * `button` - button to connect.
    ///
    /// # Post-conditions
    /// Nothing runs unless the user confirms. On confirmation `systemctl
    /// reboot` is spawned once (the button goes insensitive so a double click
    /// cannot stack two reboots); a failed spawn re-enables the button and is
    /// reported in the result label instead of leaving a dead button.
    fn wire_reboot_button(&self, button: &gtk::Button) {
        let result_label = self.result_label.clone();
        button.connect_clicked(move |button| {
            let dialog = adw::AlertDialog::builder()
                .heading(tr("Remove the installation medium"))
                .body(tr(
                    "Unplug the USB drive now, then confirm. Leaving it plugged in boots the installer again instead of Modulix OS.",
                ))
                .build();
            dialog.add_response("cancel", &tr("Cancel"));
            dialog.add_response("restart", &tr("Restart"));
            dialog.set_response_appearance("restart", adw::ResponseAppearance::Suggested);
            dialog.set_default_response(Some("cancel"));
            dialog.set_close_response("cancel");

            let result_label = result_label.clone();
            let button = button.clone();
            glib::spawn_future_local(async move {
                if dialog.choose_future(Some(&button)).await != "restart" {
                    return;
                }
                button.set_sensitive(false);
                if let Err(e) = std::process::Command::new("systemctl").arg("reboot").spawn() {
                    button.set_sensitive(true);
                    result_label.set_label(&format!("{} {e}", tr("Could not restart:")));
                }
            });
        });
    }

    /// Appends one line to the log and keeps the view pinned to the bottom,
    /// so a long `nixos-install` stays readable without scrolling by hand.
    ///
    /// * `line` - text to append; the newline is added here.
    ///
    /// # Post-conditions
    /// The buffer never grows past [`LOG_MAX_LINES`]: a full `nixos-install`
    /// emits tens of thousands of lines, and only the tail is ever useful.
    fn append_log(&self, line: &str) {
        let mut end = self.log_buffer.end_iter();
        self.log_buffer.insert(&mut end, &format!("{line}\n"));

        let overflow = self.log_buffer.line_count() - LOG_MAX_LINES;
        if overflow > 0 {
            let start = self.log_buffer.start_iter();
            if let Some(cut) = self.log_buffer.iter_at_line(overflow) {
                self.log_buffer.delete(&mut start.clone(), &mut cut.clone());
            }
        }
        let end = self.log_buffer.end_iter();
        let mark = self.log_buffer.create_mark(None, &end, false);
        self.log_view.scroll_to_mark(&mark, 0.0, false, 0.0, 0.0);
        self.log_buffer.delete_mark(&mark);
    }

    /// Switches the progress bar between pulsing and fraction mode.
    ///
    /// * `on` - true to pulse, false to go back to the last fraction.
    ///
    /// # Post-conditions
    /// At most one pulse timeout is alive at a time; turning it off removes
    /// the source rather than leaving it firing on a hidden page.
    fn set_pulsing(&self, on: bool) {
        if let Some(source) = self.pulse_source.borrow_mut().take() {
            source.remove();
        }
        if !on {
            return;
        }
        self.progress_bar.pulse();
        let progress_bar = self.progress_bar.clone();
        let source = glib::timeout_add_local(std::time::Duration::from_millis(120), move || {
            progress_bar.pulse();
            glib::ControlFlow::Continue
        });
        *self.pulse_source.borrow_mut() = Some(source);
    }

    /// Renders a failed install: the log is opened and grown, and every place
    /// the full log was persisted is named — the on-screen buffer dies with
    /// the session, the files do not.
    ///
    /// * `message` - user-facing reason.
    ///
    /// # Post-conditions
    /// The message is visible in full (wrapped, scrollable, selectable), the
    /// log expander is open and taller than during the install, and
    /// `log_path_label` is visible iff at least one persisted copy exists.
    fn show_failure(&self, message: &str) {
        self.status_label.set_label(&tr("Installation failed"));
        self.progress_bar.set_visible(false);
        self.result_label.remove_css_class("title-4");
        self.result_label.set_label(message);
        self.log_expander.set_expanded(true);
        self.log_scroller.set_min_content_height(260);

        let mut paths = Vec::new();
        if let Some(live) = install_log::path() {
            paths.push(format!("{} {live}", tr("Full log:")));
        }
        if std::path::Path::new(install_log::TARGET_LOG).exists() {
            paths.push(format!(
                "{} {}",
                tr("Copy kept on the target disk:"),
                install_log::TARGET_LOG
            ));
        }
        if paths.is_empty() {
            self.log_path_label.set_visible(false);
        } else {
            self.log_path_label.set_label(&paths.join("\n"));
            self.log_path_label.set_visible(true);
        }

        *self.failure.borrow_mut() = (message.to_string(), paths);
        self.qr_button.set_visible(true);
        self.quit_button.set_visible(true);
    }

    /// Connects the failure-page "Error report QR code" button: it reads the
    /// persisted log back and shows [`qr_report::build_report`]'s payload as a
    /// QR code, so a phone can carry the error off a kiosk machine that has
    /// neither a browser nor guaranteed network.
    ///
    /// # Post-conditions
    /// The log is read **asynchronously** (`load_contents_future`): it reaches
    /// tens of megabytes on a failed `nixos-install` and the GTK main loop must
    /// not block on it. The button is insensitive for the duration, so a
    /// double click cannot stack two reads. A log that cannot be read is not
    /// fatal — the report then carries the error message and the paths alone.
    fn wire_qr_button(&self) {
        let failure = self.failure.clone();
        self.qr_button.connect_clicked(move |button| {
            button.set_sensitive(false);
            let failure = failure.clone();
            let button = button.clone();
            glib::spawn_future_local(async move {
                let log = match install_log::path() {
                    Some(path) => gio::File::for_path(path)
                        .load_contents_future()
                        .await
                        .map(|(bytes, _etag)| String::from_utf8_lossy(&bytes).into_owned())
                        .unwrap_or_default(),
                    None => String::new(),
                };
                let (message, paths) = failure.borrow().clone();
                let payload = qr_report::build_report(&message, &paths, &log);
                qr_code::present_dialog(
                    &button,
                    &tr("Error report QR code"),
                    &tr(
                        "Scan this code with a phone to get the error report. The log is cut down to what a QR code can hold — the full log is in the files listed on screen.",
                    ),
                    &payload,
                );
                button.set_sensitive(true);
            });
        });
    }
}

impl Default for ProgressPage {
    fn default() -> Self {
        Self::new()
    }
}
