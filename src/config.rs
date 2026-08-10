//! `InstallConfig` — every answer collected across the wizard. Lives on the GTK
//! thread only (never crosses into the tokio runtime), hence plain `Rc<RefCell<_>>`
//! rather than `Arc<Mutex<_>>` (that's reserved for the concurrent backend side).

use std::cell::RefCell;
use std::rc::Rc;

pub type SharedConfig = Rc<RefCell<InstallConfig>>;

#[derive(Debug, Clone, Default)]
pub struct InstallConfig {
    pub narrator_enabled: bool,
    /// Locale identifier, e.g. `"fr_FR.UTF-8"`.
    pub language: String,
    /// IANA timezone name, e.g. `"Europe/Paris"`.
    pub timezone: String,
    pub keyboard_layout: String,
    pub keyboard_variant: String,
    pub console_keymap: String,
    pub accessibility: AccessibilityConfig,
    pub network: NetworkConfig,
    pub partitioning: PartitioningConfig,
    pub user: UserConfig,
    pub desktop_environment: DesktopEnvironment,
}

impl InstallConfig {
    pub fn new_shared() -> SharedConfig {
        Rc::new(RefCell::new(InstallConfig::default()))
    }
}

#[derive(Debug, Clone, Default)]
pub struct AccessibilityConfig {
    pub high_contrast: bool,
    pub large_text: bool,
    pub screen_magnifier: bool,
    pub sticky_keys: bool,
}

#[derive(Debug, Clone, Default)]
pub struct NetworkConfig {
    pub connected: bool,
    pub connection_name: Option<String>,
    pub behind_captive_portal: bool,
    /// Set when the user hit "Continue anyway" on `Limited`/`Unknown`
    /// connectivity — the summary screen should flag this explicitly.
    pub proceeded_without_internet: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PartitionMode {
    #[default]
    EntireDisk,
    AlongsideWindows,
    FreeSpace,
    Manual,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SwapMode {
    #[default]
    None,
    Standard,
    Hibernation,
}

#[derive(Debug, Clone, Default)]
pub struct PartitioningConfig {
    pub mode: PartitionMode,
    pub target_disk: Option<String>,
    /// New size the Windows partition should shrink to, `AlongsideWindows` mode only.
    pub shrink_to_bytes: Option<u64>,
    pub selected_free_space_id: Option<String>,
    pub swap_mode: SwapMode,
    pub encryption_enabled: bool,
    /// Deliberately separate from `UserConfig::password` — reusing the login
    /// password as the disk passphrase would mean one leak compromises both.
    pub encryption_passphrase: String,
    pub tpm2_enabled: bool,
    pub tpm2_pin: Option<String>,
    /// `Manual` mode only — one entry per existing partition the user
    /// touched, or new partition carved out of free space, in disk order.
    /// Validated entirely inside `engine::plan::plan`, see
    /// `engine::plan::ManualItem`.
    pub manual: Vec<crate::engine::plan::ManualItem>,
}

#[derive(Debug, Clone, Default)]
pub struct UserConfig {
    pub username: String,
    pub full_name: String,
    pub password: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DesktopEnvironment {
    #[default]
    Gnome,
    Plasma,
    Lxqt,
}

impl DesktopEnvironment {
    pub const ALL: [DesktopEnvironment; 3] = [
        DesktopEnvironment::Gnome,
        DesktopEnvironment::Plasma,
        DesktopEnvironment::Lxqt,
    ];

    /// Matches the `desktop` string modulix-core-utils' `init::configuration_nix` expects.
    /// Consumed by `engine::tasks::InitConfigTask` once `init_all` lands (iteration 2).
    #[allow(dead_code)]
    pub fn as_nix_str(self) -> &'static str {
        match self {
            DesktopEnvironment::Gnome => "gnome",
            DesktopEnvironment::Plasma => "plasma",
            DesktopEnvironment::Lxqt => "lxqt",
        }
    }

    pub fn screenshot_resource(self) -> &'static str {
        match self {
            DesktopEnvironment::Gnome => "/org/modulix/installer/images/screenshots/gnome.svg",
            DesktopEnvironment::Plasma => "/org/modulix/installer/images/screenshots/plasma.svg",
            DesktopEnvironment::Lxqt => "/org/modulix/installer/images/screenshots/lxqt.svg",
        }
    }

    /// Proper display name of the desktop environment — never passed to `tr()`.
    pub fn de_name(self) -> &'static str {
        match self {
            DesktopEnvironment::Gnome => "GNOME",
            DesktopEnvironment::Plasma => "KDE Plasma",
            DesktopEnvironment::Lxqt => "LXQt",
        }
    }
}
