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

    async fn apply_keyboard_layout(&self, layout: &str, variant: &str) -> mx::Result<()> {
        let mut cmd = tokio::process::Command::new("setxkbmap");
        cmd.arg("-layout").arg(layout);
        if !variant.is_empty() {
            cmd.arg("-variant").arg(variant);
        }
        let status = cmd.status().await?;
        if !status.success() {
            return Err(mx::Error::Backend(format!(
                "setxkbmap exited with {status}"
            )));
        }
        Ok(())
    }
}
