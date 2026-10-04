pub mod validity;

pub mod accessibility;
pub mod app_pack;
pub mod desktop_environment;
pub mod keyboard;
pub mod language;
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

/// Fired by the language step with the locale code it just switched to, after
/// the retranslate pass. Lets a later step derive a default from the language
/// without holding the shared [`InstallConfig`] — today only the keyboard step
/// (see `keyboard::KeyboardStep::locale_hook`).
pub type LanguageHook = Rc<RefCell<Box<dyn Fn(&str)>>>;

pub fn new_language_hook() -> LanguageHook {
    Rc::new(RefCell::new(Box::new(|_| {})))
}

/// Fired by a step that supplies its own advancement control instead of the
/// outer "Next" button (see [`Step::shows_next`]) — currently just the
/// desktop-environment step's zoom-dialog "Choose this desktop environment"
/// button.
pub type AdvanceHook = Rc<RefCell<Box<dyn Fn()>>>;

pub fn new_advance_hook() -> AdvanceHook {
    Rc::new(RefCell::new(Box::new(|| {})))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StepId {
    Accessibility,
    Language,
    Timezone,
    Keyboard,
    Network,
    Partitioning,
    User,
    DesktopEnvironment,
    AppPack,
}

impl StepId {
    pub const ALL: [StepId; 9] = [
        StepId::Accessibility,
        StepId::Language,
        StepId::Timezone,
        StepId::Keyboard,
        StepId::Network,
        StepId::Partitioning,
        StepId::User,
        StepId::DesktopEnvironment,
        StepId::AppPack,
    ];

    pub fn tag(self) -> &'static str {
        match self {
            StepId::Accessibility => "accessibility",
            StepId::Language => "language",
            StepId::Timezone => "timezone",
            StepId::Keyboard => "keyboard",
            StepId::Network => "network",
            StepId::Partitioning => "partitioning",
            StepId::User => "user",
            StepId::DesktopEnvironment => "desktop-environment",
            StepId::AppPack => "app-pack",
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
    /// `false` when the page supplies its own advancement control instead of
    /// the outer "Next" button — currently only the desktop-environment step
    /// (the zoom dialog's "Choose this desktop environment" button).
    fn shows_next(&self) -> bool {
        true
    }
}

/// Builds the 9 steps in their fixed order. `runtime` is the tokio handle
/// backend calls are dispatched onto (see `src/bridge.rs`). `retranslate_hook`
/// is empty until the caller fills it in once every step is built (see
/// [`new_retranslate_hook`]); `advance_hook` likewise (see [`new_advance_hook`]).
pub fn build_registry(
    backends: &Backends,
    runtime: &tokio::runtime::Handle,
    retranslate_hook: RetranslateHook,
    advance_hook: AdvanceHook,
    a11y: &A11ySettings,
) -> Vec<Box<dyn Step>> {
    // Built out of order on purpose: the language step needs a hook that only
    // exists once the keyboard step does. The `vec!` below still assembles
    // them in `StepId::ALL` order.
    let keyboard = keyboard::KeyboardStep::new(backends, runtime);
    let language_hook = new_language_hook();
    *language_hook.borrow_mut() = keyboard.locale_hook();

    let registry: Vec<Box<dyn Step>> = vec![
        Box::new(accessibility::AccessibilityStep::new(
            backends, runtime, a11y,
        )) as Box<dyn Step>,
        Box::new(language::LanguageStep::new(
            backends,
            runtime,
            retranslate_hook,
            language_hook,
        )) as Box<dyn Step>,
        Box::new(timezone::TimezoneStep::new(backends, runtime)) as Box<dyn Step>,
        Box::new(keyboard) as Box<dyn Step>,
        Box::new(network::NetworkStep::new(backends, runtime)) as Box<dyn Step>,
        Box::new(partitioning::PartitioningStep::new(backends, runtime, a11y)) as Box<dyn Step>,
        Box::new(user::UserStep::new()) as Box<dyn Step>,
        Box::new(desktop_environment::DesktopEnvironmentStep::new(
            advance_hook.clone(),
        )) as Box<dyn Step>,
        Box::new(app_pack::AppPackStep::new(advance_hook)) as Box<dyn Step>,
    ];
    debug_assert!(
        registry.iter().map(|s| s.id()).eq(StepId::ALL),
        "step registry order must match StepId::ALL"
    );
    registry
}
