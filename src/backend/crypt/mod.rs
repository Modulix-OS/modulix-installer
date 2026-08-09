mod cryptsetup;
mod fake;

pub use cryptsetup::CryptsetupBackend;
pub use fake::FakeCryptBackend;

use crate::mx;
use async_trait::async_trait;

/// LUKS2 encryption + TPM2 enrollment for the partitioning step (step 7,
/// stub for iteration 1 — see CLAUDE.md's TPM2 + Limine caveat before wiring
/// this into a real install).
#[async_trait]
pub trait CryptBackend: Send + Sync {
    async fn luks_format(&self, device: &str, passphrase: &str) -> mx::Result<()>;
    async fn luks_open(&self, device: &str, mapper_name: &str, passphrase: &str) -> mx::Result<()>;
    async fn enroll_tpm2(&self, device: &str, pin: Option<&str>) -> mx::Result<()>;
}
