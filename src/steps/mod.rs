pub mod validity;

pub mod accessibility;
pub mod desktop_environment;
pub mod keyboard;
pub mod language;
pub mod narrator;
pub mod network;
pub mod partitioning;
pub mod timezone;
pub mod user;

pub use validity::ValidityTracker;

use crate::a11y::A11ySettings;
use crate::backend::Backends;
use crate::config::InstallConfig;
use crate::mx;
use std::cell::RefCell;
use std::rc::Rc;

/// Fired by the language step after every locale change; the app wires it up
/// once all 9 steps exist, to call [`Step::retranslate`] on each of them.
pub type RetranslateHook = Rc<RefCell<Box<dyn Fn()>>>;

pub fn new_retranslate_hook() -> RetranslateHook {
    Rc::new(RefCell::new(Box::new(|| {})))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StepId {
    Narrator,
    Language,
    Timezone,
    Keyboard,
    Accessibility,
    Network,
    Partitioning,
    User,
    DesktopEnvironment,
}

impl StepId {
    pub const ALL: [StepId; 9] = [
        StepId::Narrator,
        StepId::Language,
        StepId::Timezone,
        StepId::Keyboard,
        StepId::Accessibility,
        StepId::Network,
        StepId::Partitioning,
        StepId::User,
        StepId::DesktopEnvironment,
    ];

    pub fn tag(self) -> &'static str {
        match self {
            StepId::Narrator => "narrator",
            StepId::Language => "language",
            StepId::Timezone => "timezone",
            StepId::Keyboard => "keyboard",
            StepId::Accessibility => "accessibility",
            StepId::Network => "network",
            StepId::Partitioning => "partitioning",
            StepId::User => "user",
            StepId::DesktopEnvironment => "desktop-environment",
        }
    }
}

/// One page of the wizard. Widgets hold their own local UI state and only
/// write it into the shared [`InstallConfig`] from [`Step::commit`], called
/// once by the "Next" button — this keeps "what the user typed" and "what
/// we've committed" clearly separate instead of live-binding every keystroke.
pub trait Step {
    fn id(&self) -> StepId;
    fn title(&self) -> String;
    fn icon_name(&self) -> &'static str;
    /// Built once by `new()` and cached; every call returns the same widget.
    fn widget(&self) -> gtk::Widget;
    fn is_relevant(&self, _cfg: &InstallConfig) -> bool {
        true
    }
    fn validity(&self) -> ValidityTracker;
    fn commit(&self, cfg: &mut InstallConfig) -> mx::Result<()>;
    /// Called after a language change on every already-built step.
    fn retranslate(&self);
}

/// Builds the 9 steps in their fixed order. `runtime` is the tokio handle
/// backend calls are dispatched onto (see `src/bridge.rs`). `retranslate_hook`
/// is empty until the caller fills it in once every step is built (see
/// [`new_retranslate_hook`]).
pub fn build_registry(
    backends: &Backends,
    runtime: &tokio::runtime::Handle,
    retranslate_hook: RetranslateHook,
    a11y: &A11ySettings,
) -> Vec<Box<dyn Step>> {
    let registry: Vec<Box<dyn Step>> = vec![
        Box::new(narrator::NarratorStep::new(backends, runtime, a11y)) as Box<dyn Step>,
        Box::new(language::LanguageStep::new(
            backends,
            runtime,
            retranslate_hook,
        )) as Box<dyn Step>,
        Box::new(timezone::TimezoneStep::new(backends, runtime)) as Box<dyn Step>,
        Box::new(keyboard::KeyboardStep::new(backends, runtime)) as Box<dyn Step>,
        Box::new(accessibility::AccessibilityStep::new(a11y)) as Box<dyn Step>,
        Box::new(network::NetworkStep::new(backends, runtime)) as Box<dyn Step>,
        Box::new(partitioning::PartitioningStep::new(backends, runtime, a11y)) as Box<dyn Step>,
        Box::new(user::UserStep::new()) as Box<dyn Step>,
        Box::new(desktop_environment::DesktopEnvironmentStep::new()) as Box<dyn Step>,
    ];
    debug_assert!(
        registry.iter().map(|s| s.id()).eq(StepId::ALL),
        "step registry order must match StepId::ALL"
    );
    registry
}
