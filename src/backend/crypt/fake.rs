use super::CryptBackend;
use crate::mx;
use async_trait::async_trait;

pub struct FakeCryptBackend;

impl FakeCryptBackend {
    pub fn new() -> Self {
        Self
    }
}

impl Default for FakeCryptBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl CryptBackend for FakeCryptBackend {
    async fn luks_format(&self, _device: &str, _passphrase: &str) -> mx::Result<()> {
        Ok(())
    }

    async fn luks_open(
        &self,
        _device: &str,
        _mapper_name: &str,
        _passphrase: &str,
    ) -> mx::Result<()> {
        Ok(())
    }

    async fn enroll_tpm2(&self, _device: &str, _pin: Option<&str>) -> mx::Result<()> {
        Ok(())
    }
}
