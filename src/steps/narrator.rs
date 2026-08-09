use crate::backend::Backends;
use crate::bridge;
use crate::config::InstallConfig;
use crate::i18n::tr;
use crate::mx;
use crate::steps::{Step, StepId, ValidityTracker};
use adw::prelude::*;
use std::cell::Cell;
use std::rc::Rc;

/// First page — narrator toggle, so speech feedback is available before
/// anything else in the wizard needs to be read aloud.
pub struct NarratorStep {
    widget: gtk::Widget,
    group: adw::PreferencesGroup,
    switch_row: adw::SwitchRow,
    enabled: Rc<Cell<bool>>,
    validity: ValidityTracker,
}

impl NarratorStep {
    pub fn new(backends: &Backends, runtime: &tokio::runtime::Handle) -> Self {
        let enabled = Rc::new(Cell::new(false));

        let switch_row = adw::SwitchRow::builder()
            .title(tr("Enable narrator"))
            .subtitle(tr(
                "Reads each page aloud as you move through the installer",
            ))
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

        let a11y = backends.a11y.clone();
        let runtime_handle = runtime.clone();
        let enabled_state = enabled.clone();
        switch_row.connect_active_notify(move |row| {
            let is_active = row.is_active();
            enabled_state.set(is_active);
            let a11y = a11y.clone();
            bridge::spawn(
                &runtime_handle,
                async move { a11y.set_narrator_enabled(is_active).await },
                |result| {
                    if let Err(e) = result {
                        eprintln!("failed to toggle narrator: {e}");
                    }
                },
            );
        });

        Self {
            widget: page.upcast(),
            group,
            switch_row,
            enabled,
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
        cfg.narrator_enabled = self.enabled.get();
        Ok(())
    }

    fn retranslate(&self) {
        self.group.set_title(&tr("Narrator"));
        self.group.set_description(Some(&tr(
            "Turn this on now if you need speech feedback for the rest of the installer",
        )));
        self.switch_row.set_title(&tr("Enable narrator"));
        self.switch_row.set_subtitle(&tr(
            "Reads each page aloud as you move through the installer",
        ));
    }
}
