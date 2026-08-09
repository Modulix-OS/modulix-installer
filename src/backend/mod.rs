pub mod a11y;
pub mod crypt;
pub mod disk;
pub mod locale;
pub mod net;

use crate::mx;
use std::sync::Arc;

/// Every backend the wizard needs, bundled once at startup. Each field is an
/// `Arc<dyn Trait>` (not `Rc`) because these cross into `tokio::spawn` — see
/// `src/bridge.rs` and CLAUDE.md's concurrency model.
#[derive(Clone)]
pub struct Backends {
    pub disk: Arc<dyn disk::DiskBackend>,
    pub network: Arc<dyn net::NetworkBackend>,
    pub locale: Arc<dyn locale::LocaleBackend>,
    pub a11y: Arc<dyn a11y::A11yBackend>,
    pub crypt: Arc<dyn crypt::CryptBackend>,
}

impl Backends {
    pub fn fake() -> Self {
        Self {
            disk: Arc::new(disk::FakeDiskBackend::new()),
            network: Arc::new(net::FakeNetworkBackend::new()),
            locale: Arc::new(locale::FakeLocaleBackend::new()),
            a11y: Arc::new(a11y::FakeA11yBackend::new()),
            crypt: Arc::new(crypt::FakeCryptBackend::new()),
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
        })
    }
}
