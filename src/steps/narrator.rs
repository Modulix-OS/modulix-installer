use crate::a11y::A11ySettings;
use crate::backend::Backends;
use crate::bridge;
use crate::config::InstallConfig;
use crate::i18n::tr;
use crate::mx;
use crate::steps::{Step, StepId, ValidityTracker};
use adw::prelude::*;

fn subtitle_available() -> String {
    tr("Reads each page aloud as you move through the installer")
}

fn subtitle_unavailable() -> String {
    tr("Unavailable: orca wasn't found on this system")
}

/// First page — narrator toggle, so speech feedback is available before
/// anything else in the wizard needs to be read aloud.
pub struct NarratorStep {
    widget: gtk::Widget,
    group: adw::PreferencesGroup,
    switch_row: adw::SwitchRow,
    a11y: A11ySettings,
    validity: ValidityTracker,
}

impl NarratorStep {
    pub fn new(backends: &Backends, runtime: &tokio::runtime::Handle, a11y: &A11ySettings) -> Self {
        let switch_row = adw::SwitchRow::builder()
            .title(tr("Enable narrator"))
            .subtitle(subtitle_available())
            .build();

        let group = adw::PreferencesGroup::builder()
            .title(tr("Narrator"))
            .description(tr(
                "Turn this on now if you need speech feedback for the rest of the installer",
            ))
            .build();
        group.add(&switch_row);

        let page = adw::PreferencesPage::new();
        page.add(&group);

        switch_row
            .bind_property("active", a11y, "narrator")
            .bidirectional()
            .sync_create()
            .build();

        {
            let a11y_backend = backends.a11y.clone();
            let switch_row = switch_row.clone();
            bridge::spawn(
                runtime,
                async move { a11y_backend.narrator_available().await },
                move |available| {
                    if !available {
                        switch_row.set_active(false);
                        switch_row.set_sensitive(false);
                        switch_row.set_subtitle(&subtitle_unavailable());
                    }
                },
            );
        }

        Self {
            widget: page.upcast(),
            group,
            switch_row,
            a11y: a11y.clone(),
            validity: ValidityTracker::ready(),
        }
    }
}

impl Step for NarratorStep {
    fn id(&self) -> StepId {
        StepId::Narrator
    }

    fn title(&self) -> String {
        tr("Narrator")
    }

    fn icon_name(&self) -> &'static str {
        "narrator-symbolic"
    }

    fn widget(&self) -> gtk::Widget {
        self.widget.clone()
    }

    fn validity(&self) -> ValidityTracker {
        self.validity.clone()
    }

    fn commit(&self, cfg: &mut InstallConfig) -> mx::Result<()> {
        cfg.narrator_enabled = self.a11y.narrator();
        Ok(())
    }

    fn retranslate(&self) {
        self.group.set_title(&tr("Narrator"));
        self.group.set_description(Some(&tr(
            "Turn this on now if you need speech feedback for the rest of the installer",
        )));
        self.switch_row.set_title(&tr("Enable narrator"));
        self.switch_row
            .set_subtitle(&if self.switch_row.is_sensitive() {
                subtitle_available()
            } else {
                subtitle_unavailable()
            });
    }
}
