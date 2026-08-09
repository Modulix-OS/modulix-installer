use crate::backend::Backends;
use crate::bridge;
use crate::config::{AccessibilityConfig, InstallConfig};
use crate::i18n::tr;
use crate::mx;
use crate::steps::{Step, StepId, ValidityTracker};
use adw::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;

pub struct AccessibilityStep {
    widget: gtk::Widget,
    group: adw::PreferencesGroup,
    high_contrast_row: adw::SwitchRow,
    large_text_row: adw::SwitchRow,
    magnifier_row: adw::SwitchRow,
    sticky_keys_row: adw::SwitchRow,
    state: Rc<RefCell<AccessibilityConfig>>,
    validity: ValidityTracker,
}

fn toggle_row(title: String, subtitle: String) -> adw::SwitchRow {
    adw::SwitchRow::builder()
        .title(title)
        .subtitle(subtitle)
        .build()
}

impl AccessibilityStep {
    pub fn new(backends: &Backends, runtime: &tokio::runtime::Handle) -> Self {
        let state = Rc::new(RefCell::new(AccessibilityConfig::default()));

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

        let group = adw::PreferencesGroup::builder()
            .title(tr("Accessibility"))
            .build();
        group.add(&high_contrast_row);
        group.add(&large_text_row);
        group.add(&magnifier_row);
        group.add(&sticky_keys_row);

        let page = adw::PreferencesPage::new();
        page.add(&group);

        wire_toggle(
            &high_contrast_row,
            backends.a11y.clone(),
            runtime.clone(),
            state.clone(),
            |a11y, enabled| Box::pin(async move { a11y.set_high_contrast(enabled).await }),
            |cfg, enabled| cfg.high_contrast = enabled,
        );

        wire_toggle(
            &large_text_row,
            backends.a11y.clone(),
            runtime.clone(),
            state.clone(),
            |a11y, enabled| Box::pin(async move { a11y.set_large_text(enabled).await }),
            |cfg, enabled| cfg.large_text = enabled,
        );

        wire_toggle(
            &magnifier_row,
            backends.a11y.clone(),
            runtime.clone(),
            state.clone(),
            |a11y, enabled| Box::pin(async move { a11y.set_screen_magnifier(enabled).await }),
            |cfg, enabled| cfg.screen_magnifier = enabled,
        );

        wire_toggle(
            &sticky_keys_row,
            backends.a11y.clone(),
            runtime.clone(),
            state.clone(),
            |a11y, enabled| Box::pin(async move { a11y.set_sticky_keys(enabled).await }),
            |cfg, enabled| cfg.sticky_keys = enabled,
        );

        Self {
            widget: page.upcast(),
            group,
            high_contrast_row,
            large_text_row,
            magnifier_row,
            sticky_keys_row,
            state,
            validity: ValidityTracker::ready(),
        }
    }
}

type A11yCall = fn(
    std::sync::Arc<dyn crate::backend::a11y::A11yBackend>,
    bool,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = mx::Result<()>> + Send>>;

fn wire_toggle(
    row: &adw::SwitchRow,
    a11y: std::sync::Arc<dyn crate::backend::a11y::A11yBackend>,
    runtime: tokio::runtime::Handle,
    state: Rc<RefCell<AccessibilityConfig>>,
    call: A11yCall,
    apply: fn(&mut AccessibilityConfig, bool),
) {
    row.connect_active_notify(move |row| {
        let enabled = row.is_active();
        apply(&mut state.borrow_mut(), enabled);
        let a11y = a11y.clone();
        let fut = call(a11y, enabled);
        bridge::spawn(&runtime, fut, |result| {
            if let Err(e) = result {
                eprintln!("failed to apply accessibility setting: {e}");
            }
        });
    });
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
        cfg.accessibility = self.state.borrow().clone();
        Ok(())
    }

    fn retranslate(&self) {
        self.group.set_title(&tr("Accessibility"));
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
    }
}
