use super::A11yBackend;
use crate::mx;
use async_trait::async_trait;

pub struct FakeA11yBackend;

impl FakeA11yBackend {
    pub fn new() -> Self {
        Self
    }
}

impl Default for FakeA11yBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl A11yBackend for FakeA11yBackend {
    async fn set_narrator_enabled(&self, _enabled: bool) -> mx::Result<()> {
        Ok(())
    }

    async fn set_high_contrast(&self, _enabled: bool) -> mx::Result<()> {
        Ok(())
    }

    async fn set_large_text(&self, _enabled: bool) -> mx::Result<()> {
        Ok(())
    }

    async fn set_screen_magnifier(&self, _enabled: bool) -> mx::Result<()> {
        Ok(())
    }

    async fn set_sticky_keys(&self, _enabled: bool) -> mx::Result<()> {
        Ok(())
    }
}
