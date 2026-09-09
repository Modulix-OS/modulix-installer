use crate::a11y::A11ySettings;
use crate::backend::Backends;
use crate::bridge;
use crate::config::InstallConfig;
use crate::i18n::tr;
use crate::mx;
use crate::steps::{Step, StepId, ValidityTracker};
use adw::prelude::*;

fn narrator_subtitle_available() -> String {
    tr("Reads each page aloud as you move through the installer")
}

fn narrator_subtitle_unavailable() -> String {
    tr("Unavailable: orca wasn't found on this system")
}

/// First page — accessibility toggles, including the narrator, so speech
/// feedback and every other assistive setting is available before anything
/// else in the wizard needs to be read aloud.
pub struct AccessibilityStep {
    widget: gtk::Widget,
    narrator_group: adw::PreferencesGroup,
    narrator_row: adw::SwitchRow,
    applied_now_group: adw::PreferencesGroup,
    applied_installed_group: adw::PreferencesGroup,
    high_contrast_row: adw::SwitchRow,
    large_text_row: adw::SwitchRow,
    magnifier_row: adw::SwitchRow,
    sticky_keys_row: adw::SwitchRow,
    preview_label: gtk::Label,
    a11y: A11ySettings,
    validity: ValidityTracker,
}

fn toggle_row(title: String, subtitle: String) -> adw::SwitchRow {
    adw::SwitchRow::builder()
        .title(title)
        .subtitle(subtitle)
        .build()
}

fn bind(row: &adw::SwitchRow, settings: &A11ySettings, property: &str) {
    row.bind_property("active", settings, property)
        .bidirectional()
        .sync_create()
        .build();
}

impl AccessibilityStep {
    pub fn new(backends: &Backends, runtime: &tokio::runtime::Handle, a11y: &A11ySettings) -> Self {
        let header_image = gtk::Image::from_icon_name("accessibility-symbolic");
        header_image.set_pixel_size(96);
        header_image.set_margin_top(12);
        header_image.set_margin_bottom(12);

        let narrator_row = adw::SwitchRow::builder()
            .title(tr("Enable narrator"))
            .subtitle(narrator_subtitle_available())
            .build();
        let narrator_group = adw::PreferencesGroup::builder()
            .title(tr("Narrator"))
            .description(tr(
                "Turn this on now if you need speech feedback for the rest of the installer",
            ))
            .build();
        narrator_group.add(&narrator_row);

        let high_contrast_row = toggle_row(
            tr("High contrast"),
            tr("Increase contrast between text and background"),
        );
        let large_text_row = toggle_row(tr("Large text"), tr("Increase the interface text size"));
        let magnifier_row = toggle_row(
            tr("Screen magnifier"),
            tr("Zoom in on the area around the cursor"),
        );
        let sticky_keys_row = toggle_row(
            tr("Sticky keys"),
            tr("Press modifier keys one at a time instead of together"),
        );

        let preview_label = gtk::Label::new(Some(&tr("Preview text")));
        preview_label.set_margin_top(6);
        preview_label.set_margin_bottom(6);

        let applied_now_group = adw::PreferencesGroup::builder()
            .title(tr("Applied now"))
            .description(tr("These take effect immediately in the installer itself"))
            .build();
        applied_now_group.add(&high_contrast_row);
        applied_now_group.add(&large_text_row);
        applied_now_group.add(&preview_label);

        let applied_installed_group = adw::PreferencesGroup::builder()
            .title(tr("Applied to the installed system"))
            .description(tr(
                "These take effect after installation, not in the installer",
            ))
            .build();
        applied_installed_group.add(&magnifier_row);
        applied_installed_group.add(&sticky_keys_row);

        let page = adw::PreferencesPage::new();
        page.add(&narrator_group);
        page.add(&applied_now_group);
        page.add(&applied_installed_group);

        let outer = gtk::Box::new(gtk::Orientation::Vertical, 0);
        outer.append(&header_image);
        outer.append(&page);
        outer.set_vexpand(true);
        header_image.set_halign(gtk::Align::Center);

        bind(&narrator_row, a11y, "narrator");
        bind(&high_contrast_row, a11y, "high-contrast");
        bind(&large_text_row, a11y, "large-text");
        bind(&magnifier_row, a11y, "screen-magnifier");
        bind(&sticky_keys_row, a11y, "sticky-keys");

        {
            let a11y_backend = backends.a11y.clone();
            let narrator_row = narrator_row.clone();
            bridge::spawn(
                runtime,
                async move { a11y_backend.narrator_available().await },
                move |available| {
                    if !available {
                        narrator_row.set_active(false);
                        narrator_row.set_sensitive(false);
                        narrator_row.set_subtitle(&narrator_subtitle_unavailable());
                    }
                },
            );
        }

        Self {
            widget: outer.upcast(),
            narrator_group,
            narrator_row,
            applied_now_group,
            applied_installed_group,
            high_contrast_row,
            large_text_row,
            magnifier_row,
            sticky_keys_row,
            preview_label,
            a11y: a11y.clone(),
            validity: ValidityTracker::ready(),
        }
    }
}

impl Step for AccessibilityStep {
    fn id(&self) -> StepId {
        StepId::Accessibility
    }

    fn title(&self) -> String {
        tr("Accessibility")
    }

    fn icon_name(&self) -> &'static str {
        "accessibility-symbolic"
    }

    fn widget(&self) -> gtk::Widget {
        self.widget.clone()
    }

    fn validity(&self) -> ValidityTracker {
        self.validity.clone()
    }

    fn commit(&self, cfg: &mut InstallConfig) -> mx::Result<()> {
        cfg.narrator_enabled = self.a11y.narrator();
        cfg.accessibility.high_contrast = self.a11y.high_contrast();
        cfg.accessibility.large_text = self.a11y.large_text();
        cfg.accessibility.screen_magnifier = self.a11y.screen_magnifier();
        cfg.accessibility.sticky_keys = self.a11y.sticky_keys();
        Ok(())
    }

    fn retranslate(&self) {
        self.narrator_group.set_title(&tr("Narrator"));
        self.narrator_group.set_description(Some(&tr(
            "Turn this on now if you need speech feedback for the rest of the installer",
        )));
        self.narrator_row.set_title(&tr("Enable narrator"));
        self.narrator_row
            .set_subtitle(&if self.narrator_row.is_sensitive() {
                narrator_subtitle_available()
            } else {
                narrator_subtitle_unavailable()
            });
        self.applied_now_group.set_title(&tr("Applied now"));
        self.applied_now_group.set_description(Some(&tr(
            "These take effect immediately in the installer itself",
        )));
        self.applied_installed_group
            .set_title(&tr("Applied to the installed system"));
        self.applied_installed_group.set_description(Some(&tr(
            "These take effect after installation, not in the installer",
        )));
        self.high_contrast_row.set_title(&tr("High contrast"));
        self.high_contrast_row
            .set_subtitle(&tr("Increase contrast between text and background"));
        self.large_text_row.set_title(&tr("Large text"));
        self.large_text_row
            .set_subtitle(&tr("Increase the interface text size"));
        self.magnifier_row.set_title(&tr("Screen magnifier"));
        self.magnifier_row
            .set_subtitle(&tr("Zoom in on the area around the cursor"));
        self.sticky_keys_row.set_title(&tr("Sticky keys"));
        self.sticky_keys_row
            .set_subtitle(&tr("Press modifier keys one at a time instead of together"));
        self.preview_label.set_label(&tr("Preview text"));
    }
}
