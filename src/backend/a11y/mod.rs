mod orca;

pub use orca::OrcaBackend;

use crate::mx;
use async_trait::async_trait;

/// Narrator + visual/keyboard accessibility toggles (step 1).
/// `set_narrator_enabled` is the one method that actually does something
/// under `cage` (spawns/kills orca). The other four are best-effort
/// `gsettings` calls against the GNOME a11y schemas: on a dev machine under
/// GNOME they make the desktop follow the installer's toggles, but under the
/// kiosk session there's no GNOME Shell/dconf daemon listening, so
/// implementations must treat a missing `gsettings` binary as success rather
/// than failing the whole step (see `src/a11y.rs` for what actually applies
/// live to the installer itself: high contrast and large text).
#[async_trait]
pub trait A11yBackend: Send + Sync {
    async fn set_narrator_enabled(&self, enabled: bool) -> mx::Result<()>;
    /// Resolves `orca` in `PATH` so the narrator step can grey out its row
    /// and explain why, instead of silently failing to spawn it.
    async fn narrator_available(&self) -> bool;
    async fn set_high_contrast(&self, enabled: bool) -> mx::Result<()>;
    async fn set_large_text(&self, enabled: bool) -> mx::Result<()>;
    async fn set_screen_magnifier(&self, enabled: bool) -> mx::Result<()>;
    async fn set_sticky_keys(&self, enabled: bool) -> mx::Result<()>;
}
