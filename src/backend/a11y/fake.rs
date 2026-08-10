use super::A11yBackend;
use crate::mx;
use async_trait::async_trait;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Default)]
pub struct FakeA11yBackend {
    narrator: AtomicBool,
    high_contrast: AtomicBool,
    large_text: AtomicBool,
    screen_magnifier: AtomicBool,
    sticky_keys: AtomicBool,
}

impl FakeA11yBackend {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl A11yBackend for FakeA11yBackend {
    async fn set_narrator_enabled(&self, enabled: bool) -> mx::Result<()> {
        self.narrator.store(enabled, Ordering::Relaxed);
        Ok(())
    }

    async fn narrator_available(&self) -> bool {
        true
    }

    async fn set_high_contrast(&self, enabled: bool) -> mx::Result<()> {
        self.high_contrast.store(enabled, Ordering::Relaxed);
        Ok(())
    }

    async fn set_large_text(&self, enabled: bool) -> mx::Result<()> {
        self.large_text.store(enabled, Ordering::Relaxed);
        Ok(())
    }

    async fn set_screen_magnifier(&self, enabled: bool) -> mx::Result<()> {
        self.screen_magnifier.store(enabled, Ordering::Relaxed);
        Ok(())
    }

    async fn set_sticky_keys(&self, enabled: bool) -> mx::Result<()> {
        self.sticky_keys.store(enabled, Ordering::Relaxed);
        Ok(())
    }
}
