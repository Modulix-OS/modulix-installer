//! In-memory `CryptBackend` for `--fake-disks=`/`--fake-error=encrypt:*`.
//! No LUKS header is ever written; `luks_open` doesn't even need to
//! remember a mapper exists, since `FakeDiskBackend::resolve_device`
//! registers it the moment `EncryptTask` asks for it.

use super::fault::Fault;
use super::{FaultPlan, require_device_node};
use crate::backend::crypt::CryptBackend;
use crate::mx;
use async_trait::async_trait;
use std::sync::Arc;

pub struct FakeCryptBackend {
    faults: Arc<FaultPlan>,
}

impl FakeCryptBackend {
    pub fn new(faults: Arc<FaultPlan>) -> Self {
        Self { faults }
    }
}

#[async_trait]
impl CryptBackend for FakeCryptBackend {
    async fn luks_format(&self, device: &str, _passphrase: &str) -> mx::Result<()> {
        require_device_node(device)?;
        if self.faults.is(Fault::EncryptLuksFormat) {
            return Err(mx::Error::Backend(
                "cryptsetup luksFormat exited with status 4".into(),
            ));
        }
        Ok(())
    }

    async fn luks_open(
        &self,
        device: &str,
        _mapper_name: &str,
        _passphrase: &str,
    ) -> mx::Result<()> {
        require_device_node(device)?;
        if self.faults.is(Fault::EncryptLuksOpen) {
            return Err(mx::Error::Backend(
                "No key available with this passphrase".into(),
            ));
        }
        Ok(())
    }

    async fn enroll_tpm2(
        &self,
        device: &str,
        _passphrase: &str,
        _pin: Option<&str>,
    ) -> mx::Result<()> {
        require_device_node(device)?;
        if self.faults.is(Fault::TpmNoDevice) {
            return Err(mx::Error::Backend("no TPM2 device found".into()));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn rejects_a_udisks2_object_path() {
        let backend = FakeCryptBackend::new(Arc::new(FaultPlan { fault: None }));
        let err = backend
            .luks_format("/org/freedesktop/UDisks2/block_devices/vda2", "secret")
            .await
            .unwrap_err();
        assert!(matches!(err, mx::Error::Backend(_)));
    }

    #[tokio::test]
    async fn luks_format_fault_trips_only_that_method() {
        let backend = FakeCryptBackend::new(Arc::new(FaultPlan {
            fault: Some(Fault::EncryptLuksFormat),
        }));
        assert!(backend.luks_format("/dev/fake0p2", "secret").await.is_err());
        assert!(
            backend
                .luks_open("/dev/fake0p2", "modulixroot", "secret")
                .await
                .is_ok()
        );
    }
}
