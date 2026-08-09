use super::A11yBackend;
use crate::mx;
use async_trait::async_trait;
use tokio::sync::Mutex;

async fn gsettings_set(schema: &str, key: &str, value: &str) -> mx::Result<()> {
    let status = tokio::process::Command::new("gsettings")
        .args(["set", schema, key, value])
        .status()
        .await?;
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
