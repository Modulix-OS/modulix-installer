use super::A11yBackend;
use crate::mx;
use async_trait::async_trait;
use tokio::sync::Mutex;

/// `true` only under a full GNOME session: no GNOME Shell/dconf daemon is
/// running under the kiosk session, so a missing `gsettings` binary — or one present
/// but with nothing to talk to — is the expected case, not an error.
fn binary_in_path(name: &str) -> bool {
    let Some(paths) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&paths)
        .any(|dir| std::fs::metadata(dir.join(name)).is_ok_and(|meta| meta.is_file()))
}

/// Best-effort: a missing `gsettings` binary succeeds silently (the normal
/// case in the kiosk session), any other failure still propagates.
async fn gsettings_set(schema: &str, key: &str, value: &str) -> mx::Result<()> {
    let status = match tokio::process::Command::new("gsettings")
        .args(["set", schema, key, value])
        .status()
        .await
    {
        Ok(status) => status,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.into()),
    };
    if !status.success() {
        return Err(mx::Error::Backend(format!(
            "gsettings set {schema} {key} {value} failed: {status}"
        )));
    }
    Ok(())
}

pub struct OrcaBackend {
    narrator: Mutex<Option<tokio::process::Child>>,
}

impl OrcaBackend {
    pub fn new() -> Self {
        Self {
            narrator: Mutex::new(None),
        }
    }
}

impl Default for OrcaBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl A11yBackend for OrcaBackend {
    async fn set_narrator_enabled(&self, enabled: bool) -> mx::Result<()> {
        let mut narrator = self.narrator.lock().await;
        if enabled {
            if narrator.is_none() {
                *narrator = Some(
                    tokio::process::Command::new("orca")
                        .arg("--replace")
                        .spawn()?,
                );
            }
        } else if let Some(mut child) = narrator.take() {
            child.kill().await?;
        }
        Ok(())
    }

    async fn narrator_available(&self) -> bool {
        binary_in_path("orca")
    }

    async fn set_high_contrast(&self, enabled: bool) -> mx::Result<()> {
        gsettings_set(
            "org.gnome.desktop.a11y.interface",
            "high-contrast",
            &enabled.to_string(),
        )
        .await
    }

    async fn set_large_text(&self, enabled: bool) -> mx::Result<()> {
        let scale = if enabled { "1.25" } else { "1.0" };
        gsettings_set("org.gnome.desktop.interface", "text-scaling-factor", scale).await
    }

    async fn set_screen_magnifier(&self, enabled: bool) -> mx::Result<()> {
        gsettings_set(
            "org.gnome.desktop.a11y.applications",
            "screen-magnifier-enabled",
            &enabled.to_string(),
        )
        .await
    }

    async fn set_sticky_keys(&self, enabled: bool) -> mx::Result<()> {
        gsettings_set(
            "org.gnome.desktop.a11y.keyboard",
            "stickykeys-enable",
            &enabled.to_string(),
        )
        .await
    }
}
