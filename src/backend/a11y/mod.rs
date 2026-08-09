mod fake;
mod orca;

pub use fake::FakeA11yBackend;
pub use orca::OrcaBackend;

use crate::mx;
use async_trait::async_trait;

/// Narrator + visual/keyboard accessibility toggles (steps 1 and 5). Real
/// impl drives orca (narrator) and the GNOME a11y GSettings schemas (the
/// kiosk session still honors them even though there's no GNOME Shell).
#[async_trait]
pub trait A11yBackend: Send + Sync {
    async fn set_narrator_enabled(&self, enabled: bool) -> mx::Result<()>;
    async fn set_high_contrast(&self, enabled: bool) -> mx::Result<()>;
    async fn set_large_text(&self, enabled: bool) -> mx::Result<()>;
    async fn set_screen_magnifier(&self, enabled: bool) -> mx::Result<()>;
    async fn set_sticky_keys(&self, enabled: bool) -> mx::Result<()>;
}
