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
}

impl Backends {
    /// Connects every backend. Async because disk and network need a D-Bus
    /// handshake; call this once at startup via `runtime.block_on`.
    ///
    /// # Post-conditions
    /// Every field talks to the real subsystem. There is no simulated
    /// alternative and no fallback: an installer that silently simulates an
    /// install and then reports success is worse than one that refuses to
    /// start, so a failure here is fatal — see `main`.
    ///
    /// # Returns
    /// The connected bundle.
    ///
    /// # Errors
    /// Whatever `Udisks2Backend::connect` or `NetworkManagerBackend::connect`
    /// returned; the three remaining backends are infallible to construct.
    pub async fn new() -> mx::Result<Self> {
        Ok(Self {
            disk: Arc::new(disk::Udisks2Backend::connect().await?),
            network: Arc::new(net::NetworkManagerBackend::connect().await?),
            locale: Arc::new(locale::SystemLocaleBackend::new()),
            a11y: Arc::new(a11y::OrcaBackend::new()),
            crypt: Arc::new(crypt::CryptsetupBackend::new()),
        })
    }
}
