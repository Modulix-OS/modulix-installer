//! `InstallConfig` — every answer collected across the wizard. Lives on the GTK
//! thread only (never crosses into the tokio runtime), hence plain `Rc<RefCell<_>>`
//! rather than `Arc<Mutex<_>>` (that's reserved for the concurrent backend side).

use std::cell::RefCell;
use std::rc::Rc;

pub type SharedConfig = Rc<RefCell<InstallConfig>>;

/// `networking.hostName` proposed by the user step and used when nothing is
/// typed. Must satisfy `steps::user::hostname::blocked_reason`.
pub const DEFAULT_HOSTNAME: &str = "modulixos";

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
    pub app_pack: AppPack,
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

#[derive(Debug, Clone)]
pub struct UserConfig {
    pub username: String,
    pub full_name: String,
    /// `networking.hostName` of the installed system. Never empty: the user
    /// step seeds it with `"modulixos"` and blocks on an empty field.
    pub hostname: String,
    pub password: String,
}

impl Default for UserConfig {
    fn default() -> Self {
        Self {
            username: String::new(),
            full_name: String::new(),
            hostname: DEFAULT_HOSTNAME.to_string(),
            password: String::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DesktopEnvironment {
    #[default]
    Gnome,
    /// Kept for `as_nix_str`/`screenshots`, not yet offered by
    /// [`DesktopEnvironment::ALL`] — hence never constructed today.
    #[allow(dead_code)]
    Plasma,
    /// Same as [`DesktopEnvironment::Plasma`], and additionally unsupported
    /// by `modulix-core-utils` (see `as_nix_str`).
    #[allow(dead_code)]
    Xfce,
    /// Same as [`DesktopEnvironment::Plasma`].
    #[allow(dead_code)]
    Lxqt,
}

impl DesktopEnvironment {
    pub const ALL: [DesktopEnvironment; 1] = [
        DesktopEnvironment::Gnome, // DesktopEnvironment::Plasma,
                                   // DesktopEnvironment::Xfce,
                                   // DesktopEnvironment::Lxqt,
    ];

    /// Matches the `desktop` string modulix-core-utils' `init::configuration_nix` expects.
    /// Consumed by `engine::tasks::InitConfigTask`, which maps it to
    /// `modulix_core_utils::init::Desktop`.
    /// NOTE: `modulix-core-utils` only validates `"gnome" | "plasma" | "lxqt"`
    /// today, so `Desktop::parse` rejects `"xfce"` — that branch has to be
    /// added there before `DesktopEnvironment::Xfce` can be offered.
    pub fn as_nix_str(self) -> &'static str {
        match self {
            DesktopEnvironment::Gnome => "gnome",
            DesktopEnvironment::Plasma => "plasma",
            DesktopEnvironment::Xfce => "xfce",
            DesktopEnvironment::Lxqt => "lxqt",
        }
    }

    /// Screenshots shown in step 9's zoom carousel, in order. The first
    /// entry is also the card thumbnail. Placeholder: every slot currently
    /// points at the same per-DE SVG — swap individual entries for real
    /// shots later, the carousel length follows this table.
    pub fn screenshots(self) -> &'static [&'static str] {
        const GNOME_SHOT: [&str; 3] = [
            "/org/modulix/installer/images/screenshots/gnome1.jpg",
            "/org/modulix/installer/images/screenshots/gnome2.jpg",
            "/org/modulix/installer/images/screenshots/gnome3.jpg",
        ];
        const PLASMA_SHOT: &str = "/org/modulix/installer/images/screenshots/plasma.svg";
        const XFCE_SHOT: &str = "/org/modulix/installer/images/screenshots/xfce.svg";
        const LXQT_SHOT: &str = "/org/modulix/installer/images/screenshots/lxqt.svg";
        match self {
            DesktopEnvironment::Gnome => &GNOME_SHOT,
            DesktopEnvironment::Plasma => &[PLASMA_SHOT, PLASMA_SHOT, PLASMA_SHOT],
            DesktopEnvironment::Xfce => &[XFCE_SHOT, XFCE_SHOT, XFCE_SHOT],
            DesktopEnvironment::Lxqt => &[LXQT_SHOT, LXQT_SHOT, LXQT_SHOT],
        }
    }

    /// Card thumbnail — first screenshot; the table is never empty.
    pub fn screenshot_resource(self) -> &'static str {
        self.screenshots()[0]
    }

    /// Proper display name of the desktop environment — never passed to `tr()`.
    pub fn de_name(self) -> &'static str {
        match self {
            DesktopEnvironment::Gnome => "GNOME",
            DesktopEnvironment::Plasma => "KDE Plasma",
            DesktopEnvironment::Xfce => "Xfce",
            DesktopEnvironment::Lxqt => "LXQt",
        }
    }

    /// File manager, terminal, PDF reader (no office suite), image viewer,
    /// archive manager, text editor, media player — one native pick per DE,
    /// consumed by [`AppPack::packages`] for the base pack.
    fn base_pack_packages(self) -> &'static [&'static str] {
        match self {
            DesktopEnvironment::Gnome => &[
                "nautilus",
                "gnome-console",
                "evince",
                "loupe",
                "file-roller",
                "gnome-text-editor",
                "celluloid",
            ],
            DesktopEnvironment::Plasma => &[
                "kdePackages.dolphin",
                "kdePackages.konsole",
                "kdePackages.okular",
                "kdePackages.gwenview",
                "kdePackages.ark",
                "kdePackages.kate",
                "kdePackages.haruna",
            ],
            DesktopEnvironment::Xfce => &[
                "xfce.thunar",
                "xfce.xfce4-terminal",
                "atril",
                "xfce.ristretto",
                "xarchiver",
                "xfce.mousepad",
                "xfce.parole",
            ],
            DesktopEnvironment::Lxqt => &[
                "lxqt.pcmanfm-qt",
                "lxqt.qterminal",
                "qpdfview",
                "lxqt.lximage-qt",
                "lxqt.lxqt-archiver",
                "featherpad",
                "vlc",
            ],
        }
    }
}

/// Independent of the desktop environment: browser + printing stack.
const COMMON_BASE_PACKAGES: &[&str] = &["firefox", "cups", "gutenprint", "system-config-printer"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AppPack {
    /// Bare system: no application package beyond the DE itself.
    None,
    #[default]
    Base,
}

impl AppPack {
    pub const ALL: [AppPack; 2] = [AppPack::None, AppPack::Base];

    /// nixpkgs packages installed on top of the DE. Consumed by
    /// `engine::tasks::ExtraConfigTask`, which renders them into `apps.nix`.
    pub fn packages(self, de: DesktopEnvironment) -> Vec<&'static str> {
        match self {
            AppPack::None => Vec::new(),
            AppPack::Base => COMMON_BASE_PACKAGES
                .iter()
                .copied()
                .chain(de.base_pack_packages().iter().copied())
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_desktop_environment_has_a_screenshot() {
        for de in DesktopEnvironment::ALL {
            assert!(!de.screenshots().is_empty());
        }
    }

    #[test]
    fn app_pack_packages_are_consistent() {
        for de in DesktopEnvironment::ALL {
            assert!(!AppPack::Base.packages(de).is_empty());
            assert!(AppPack::None.packages(de).is_empty());
        }
    }
}
