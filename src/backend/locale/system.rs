use super::catalog;
use super::{KeyboardLayout, LocaleBackend, LocaleEntry, TimezoneEntry};
use crate::mx;
use async_trait::async_trait;

pub struct SystemLocaleBackend;

impl SystemLocaleBackend {
    pub fn new() -> Self {
        Self
    }
}

impl Default for SystemLocaleBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl LocaleBackend for SystemLocaleBackend {
    async fn list_locales(&self) -> mx::Result<Vec<LocaleEntry>> {
        catalog::read_locales().await
    }

    async fn list_timezones(&self) -> mx::Result<Vec<TimezoneEntry>> {
        catalog::read_timezones().await
    }

    async fn list_keyboard_layouts(&self) -> mx::Result<Vec<KeyboardLayout>> {
        catalog::read_keyboard_layouts().await
    }

    /// Applies `layout`/`variant` to the whole running session through sway's
    /// IPC socket.
    ///
    /// The variant is cleared first: sway validates each `input` command on
    /// its own, so a stale variant left over from the previous layout would
    /// make the new `xkb_layout` an invalid combination and be rejected.
    /// The empty variant is sent as a literal `""`: `swaymsg` joins its
    /// arguments with spaces, so an empty argument would vanish and leave
    /// sway with a value-less `xkb_variant` command.
    async fn apply_keyboard_layout(&self, layout: &str, variant: &str) -> mx::Result<()> {
        swaymsg(&["input", "type:keyboard", "xkb_variant", "\"\""]).await?;
        swaymsg(&["input", "type:keyboard", "xkb_layout", layout]).await?;
        if !variant.is_empty() {
            swaymsg(&["input", "type:keyboard", "xkb_variant", variant]).await?;
        }
        Ok(())
    }
}

/// Runs one `swaymsg` command, arguments passed as an array (never a shell).
///
/// * `args` - the sway command and its operands, e.g.
///   `["input", "type:keyboard", "xkb_layout", "fr"]`.
///
/// # Pre-conditions
/// `SWAYSOCK` is set in the environment — sway exports it for the clients it
/// `exec`s, which is how the installer is started (`nix/kiosk-module.nix`).
///
/// # Returns
/// `Ok(())` once `swaymsg` exited successfully.
///
/// # Errors
/// `mx::Error::Backend` if `swaymsg` cannot be spawned (not on `PATH`, or no
/// sway session) or exits non-zero — e.g. an `xkb_layout` xkbcommon rejects.
async fn swaymsg(args: &[&str]) -> mx::Result<()> {
    let output = tokio::process::Command::new("swaymsg")
        .arg("--")
        .args(args)
        .output()
        .await
        .map_err(|e| mx::Error::Backend(format!("swaymsg could not be run: {e}")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(mx::Error::Backend(format!(
            "swaymsg {} failed: {}",
            args.join(" "),
            stderr.trim()
        )));
    }
    Ok(())
}
