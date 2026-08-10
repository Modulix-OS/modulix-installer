pub mod a11y;
pub mod crypt;
pub mod disk;
pub mod locale;
pub mod net;

use crate::mx;
use std::sync::Arc;

/// Every backend the wizard needs, bundled once at startup. Each field is an
/// `Arc<dyn Trait>` (not `Rc`) because these cross into `tokio::spawn` — see
/// `src/bridge.rs` and the concurrency model.
#[derive(Clone)]
pub struct Backends {
    pub disk: Arc<dyn disk::DiskBackend>,
    pub network: Arc<dyn net::NetworkBackend>,
    pub locale: Arc<dyn locale::LocaleBackend>,
    pub a11y: Arc<dyn a11y::A11yBackend>,
    pub crypt: Arc<dyn crypt::CryptBackend>,
    /// True under `--fake` — gates UI affordances that need a real device
    /// node to make sense, e.g. step 7's "Partition with GParted…" button
    /// (`/dev/fake0` doesn't exist for GParted to open).
    pub is_fake: bool,
}

impl Backends {
    pub fn fake() -> Self {
        Self::fake_with(disk::DiskScenario::Linux)
    }

    /// Same as [`Backends::fake`], but with the disk backend seeded from a
    /// named [`disk::DiskScenario`] instead of the default `linux` one —
    /// what `--fake-disk=<scenario>` selects.
    pub fn fake_with(scenario: disk::DiskScenario) -> Self {
        Self {
            disk: Arc::new(disk::FakeDiskBackend::with_scenario(scenario)),
            network: Arc::new(net::FakeNetworkBackend::new()),
            locale: Arc::new(locale::FakeLocaleBackend::new()),
            a11y: Arc::new(a11y::FakeA11yBackend::new()),
            crypt: Arc::new(crypt::FakeCryptBackend::new()),
            is_fake: true,
        }
    }

    /// Connects the real backends. Async because disk/network need a D-Bus
    /// handshake; call this once at startup via `runtime.block_on`.
    pub async fn real() -> mx::Result<Self> {
        Ok(Self {
            disk: Arc::new(disk::Udisks2Backend::connect().await?),
            network: Arc::new(net::NetworkManagerBackend::connect().await?),
            locale: Arc::new(locale::SystemLocaleBackend::new()),
            a11y: Arc::new(a11y::OrcaBackend::new()),
            crypt: Arc::new(crypt::CryptsetupBackend::new()),
            is_fake: false,
        })
    }
}
