use crate::a11y::A11ySettings;
use crate::config::InstallConfig;
use crate::i18n::tr;
use crate::mx;
use crate::steps::{Step, StepId, ValidityTracker};
use adw::prelude::*;

pub struct AccessibilityStep {
    widget: gtk::Widget,
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
    pub fn new(a11y: &A11ySettings) -> Self {
        let header_image = gtk::Image::from_icon_name("accessibility-symbolic");
        header_image.set_pixel_size(96);
        header_image.set_margin_top(12);
        header_image.set_margin_bottom(12);

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
        page.add(&applied_now_group);
        page.add(&applied_installed_group);

        let outer = gtk::Box::new(gtk::Orientation::Vertical, 0);
        outer.append(&header_image);
        outer.append(&page);
        outer.set_vexpand(true);
        header_image.set_halign(gtk::Align::Center);

        bind(&high_contrast_row, a11y, "high-contrast");
        bind(&large_text_row, a11y, "large-text");
        bind(&magnifier_row, a11y, "screen-magnifier");
        bind(&sticky_keys_row, a11y, "sticky-keys");

        Self {
            widget: outer.upcast(),
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
        cfg.accessibility.high_contrast = self.a11y.high_contrast();
        cfg.accessibility.large_text = self.a11y.large_text();
        cfg.accessibility.screen_magnifier = self.a11y.screen_magnifier();
        cfg.accessibility.sticky_keys = self.a11y.sticky_keys();
        Ok(())
    }

    fn retranslate(&self) {
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
