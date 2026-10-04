mod catalog;
pub(crate) mod display_name;
mod evdev;
pub mod keyboard_default;
mod system;
mod zonetab;

pub use display_name::{display_name, language_code_of};
pub use evdev::parse_evdev_xml;
pub use system::SystemLocaleBackend;
pub use zonetab::parse_zone_tab;

use crate::mx;
use async_trait::async_trait;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocaleEntry {
    /// e.g. `"fr_FR.UTF-8"` — every UTF-8 locale glibc supports (from its
    /// `SUPPORTED` catalog), not just the ones already generated on this
    /// particular system (`locale -a` is only a fallback — see `catalog.rs`).
    pub code: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TimezoneEntry {
    /// IANA zone name, e.g. `"Europe/Paris"`.
    pub name: String,
    pub latitude: f64,
    pub longitude: f64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyboardVariant {
    pub code: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyboardLayout {
    pub code: String,
    pub description: String,
    pub variants: Vec<KeyboardVariant>,
}

/// Locale/timezone/keyboard catalogs, real impl reads them straight off the
/// live ISO filesystem (`locale -a`, `zone.tab`, `evdev.xml`).
#[async_trait]
pub trait LocaleBackend: Send + Sync {
    async fn list_locales(&self) -> mx::Result<Vec<LocaleEntry>>;
    async fn list_timezones(&self) -> mx::Result<Vec<TimezoneEntry>>;
    async fn list_keyboard_layouts(&self) -> mx::Result<Vec<KeyboardLayout>>;
    /// Applies a layout/variant to the running session (the kiosk compositor's
    /// IPC), so the typing test — and every password field after it — match
    /// what the user picked. Failure must never block the wizard, but it is
    /// surfaced by the keyboard step rather than swallowed.
    async fn apply_keyboard_layout(&self, layout: &str, variant: &str) -> mx::Result<()>;
}
