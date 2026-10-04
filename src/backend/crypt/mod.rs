mod cryptsetup;

pub use cryptsetup::CryptsetupBackend;

use crate::mx;
use async_trait::async_trait;

/// LUKS2 encryption and TPM2 enrollment of the install target's containers,
/// driven by `engine::tasks::EncryptTask` and
/// `engine::tasks::EnrollTpmTask`.
///
/// Every method takes a real `/dev/...` device node, never a udisks2 object
/// path: these shell out to tools that don't speak D-Bus. Callers holding a
/// `PipelineState` path must convert it with `DiskBackend::device_node`
/// first; the implementation rejects anything else rather than letting
/// `cryptsetup` fail with its own opaque exit status.
#[async_trait]
pub trait CryptBackend: Send + Sync {
    async fn luks_format(&self, device: &str, passphrase: &str) -> mx::Result<()>;
    async fn luks_open(&self, device: &str, mapper_name: &str, passphrase: &str) -> mx::Result<()>;
    /// Enrolls a TPM2 token on an existing LUKS2 container.
    ///
    /// # Parameters
    /// * `device` - the raw container's device node.
    /// * `passphrase` - an existing passphrase of the container, needed to
    ///   unlock the volume key the new token is wrapped around.
    /// * `pin` - PIN to require in addition to the TPM2 state, or `None` for
    ///   a token that unlocks unattended.
    async fn enroll_tpm2(
        &self,
        device: &str,
        passphrase: &str,
        pin: Option<&str>,
    ) -> mx::Result<()>;
}
