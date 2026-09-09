//! Install progress screen — an auto-advancing slideshow fills the main
//! area while `Task`/`Pipeline` (see `engine`) runs in the background, log
//! tucked away in a collapsed `Expander` and the progress bar pinned to the
//! bottom. Pushed by `SummaryPage` once the user confirms the
//! destructive-action dialog.

use crate::backend::Backends;
use crate::config::InstallConfig;
use crate::engine::{self, ProgressEvent, TaskCtx};
use crate::finish::slides::INSTALL_SLIDES;
use crate::i18n::tr;
use crate::mx;
use crate::widgets::slideshow::Slideshow;
use adw::prelude::*;

fn render_backend_error(e: &mx::Error) -> String {
    match e {
        mx::Error::Backend(msgid) => tr(msgid),
        other => other.to_string(),
    }
}

#[derive(Clone)]
pub struct ProgressPage {
    page: adw::NavigationPage,
    progress_bar: gtk::ProgressBar,
    status_label: gtk::Label,
    log_buffer: gtk::TextBuffer,
    log_expander: gtk::Expander,
    result_box: gtk::Box,
    result_icon: gtk::Image,
    result_label: gtk::Label,
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

        let result_icon = gtk::Image::builder().pixel_size(32).build();
        let result_label = gtk::Label::builder()
            .wrap(true)
            .xalign(0.0)
            .hexpand(true)
            .build();
        let result_box = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        result_box.append(&result_icon);
        result_box.append(&result_label);
        result_box.set_visible(false);
        result_box.add_css_class("card");
        result_box.set_margin_top(6);
        result_box.set_margin_bottom(6);
        result_box.set_margin_start(6);
        result_box.set_margin_end(6);

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
        content.append(&result_box);
        content.append(&log_expander);
        content.append(&status_label);
        content.append(&progress_bar);

        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&adw::HeaderBar::new());
        toolbar.set_content(Some(&content));
        let page = adw::NavigationPage::new(&toolbar, &tr("Installing"));
        page.set_can_pop(false);

        Self {
            page,
            progress_bar,
            status_label,
            log_buffer,
            log_expander,
            result_box,
            result_icon,
            result_label,
            slideshow,
        }
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
        self.page.set_title(&tr("Installing"));
        self.log_expander.set_label(Some(&tr("Details")));
        self.slideshow.retranslate();
        self.slideshow.start();

        let (tx, rx) = async_channel::unbounded::<ProgressEvent>();
        let (done_tx, done_rx) = async_channel::bounded::<mx::Result<()>>(1);

        runtime.spawn(async move {
            let ctx = TaskCtx::new(backends, config);
            let result = engine::full_pipeline().run(&ctx, &tx).await;
            let _ = done_tx.send(result).await;
        });

        {
            let progress_bar = self.progress_bar.clone();
            let status_label = self.status_label.clone();
            let log_buffer = self.log_buffer.clone();
            glib::spawn_future_local(async move {
                while let Ok(event) = rx.recv().await {
                    match event {
                        ProgressEvent::Started { task } => status_label.set_label(&task),
                        ProgressEvent::Log(line) => {
                            let mut end = log_buffer.end_iter();
                            log_buffer.insert(&mut end, &format!("{line}\n"));
                        }
                        ProgressEvent::Progress { fraction } => {
                            progress_bar.set_fraction(fraction);
                        }
                        ProgressEvent::Finished { task } => {
                            let mut end = log_buffer.end_iter();
                            log_buffer.insert(&mut end, &format!("\u{2713} {task}\n"));
                        }
                    }
                }
            });
        }

        {
            let status_label = self.status_label.clone();
            let progress_bar = self.progress_bar.clone();
            let result_box = self.result_box.clone();
            let result_icon = self.result_icon.clone();
            let result_label = self.result_label.clone();
            let page = self.page.clone();
            let slideshow = self.slideshow.clone();
            glib::spawn_future_local(async move {
                let outcome = done_rx.recv().await;
                slideshow.stop();
                result_box.set_visible(true);
                match outcome {
                    Ok(Ok(())) => {
                        progress_bar.set_fraction(1.0);
                        status_label.set_label(&tr("Installation complete"));
                        result_icon.set_icon_name(Some("emblem-ok-symbolic"));
                        result_label
                            .set_label(&tr("Modulix OS is ready. You can restart into it now."));
                        page.set_can_pop(true);
                    }
                    Ok(Err(e)) => {
                        status_label.set_label(&tr("Installation failed"));
                        result_icon.set_icon_name(Some("dialog-error-symbolic"));
                        result_label.set_label(&render_backend_error(&e));
                        page.set_can_pop(true);
                    }
                    Err(_) => {
                        status_label.set_label(&tr("Installation failed"));
                        result_icon.set_icon_name(Some("dialog-error-symbolic"));
                        result_label.set_label(&tr("The installer stopped unexpectedly"));
                        page.set_can_pop(true);
                    }
                }
            });
        }
    }
}

impl Default for ProgressPage {
    fn default() -> Self {
        Self::new()
    }
}
