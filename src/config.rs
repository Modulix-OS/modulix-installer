//! `InstallConfig` — every answer collected across the wizard. Lives on the GTK
//! thread only (never crosses into the tokio runtime), hence plain `Rc<RefCell<_>>`
//! rather than `Arc<Mutex<_>>` (that's reserved for the concurrent backend side).

use std::cell::RefCell;
use std::collections::BTreeMap;
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
    pub apps: AppSelection,
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
    /// Login password of both the primary user and root. Never written into
    /// the NixOS configuration — `engine::tasks::SetPasswordsTask` pipes it
    /// to `chpasswd` after the install so it never reaches the Nix store.
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
    /// archive manager, text editor, media player, disk tools and the GNOME
    /// welcome tour — one native pick per DE, consumed by
    /// [`AppCard::packages`] for the base pack.
    fn base_pack_packages(self) -> &'static [&'static str] {
        match self {
            DesktopEnvironment::Gnome => &[
                "nautilus",
                "gnome-console",
                "papers",
                "loupe",
                "file-roller",
                "gnome-text-editor",
                "decibels",
                "showtime",
                "gnome-calculator",
                "baobab",
                "gnome-disk-utility",
                "gnome-extension-manager",
                "gnome-tour",
                "resources",
            ],
            DesktopEnvironment::Plasma => &[
                "kdePackages.dolphin",
                "kdePackages.konsole",
                "kdePackages.okular",
                "kdePackages.gwenview",
                "kdePackages.ark",
                "kdePackages.kate",
                "kdePackages.haruna",
                "kdePackages.plasma-systemmonitor",
            ],
            DesktopEnvironment::Xfce => &[
                "xfce.thunar",
                "xfce.xfce4-terminal",
                "atril",
                "xfce.ristretto",
                "xarchiver",
                "xfce.mousepad",
                "xfce.parole",
                "htop",
            ],
            DesktopEnvironment::Lxqt => &[
                "lxqt.pcmanfm-qt",
                "lxqt.qterminal",
                "qpdfview",
                "lxqt.lximage-qt",
                "lxqt.lxqt-archiver",
                "featherpad",
                "vlc",
                "htop",
            ],
        }
    }
}

/// Independent of the desktop environment. Two of the base pack's staples are
/// deliberately absent, because an `environment.systemPackages` entry cannot
/// express them: printing needs `services.printing.enable` plus a driver set,
/// and Firefox needs its language packs, hunspell dictionaries, VAAPI
/// preferences and GNOME search provider. Both come in as mxpkgs modules
/// instead — see [`AppCard::modules`].
const COMMON_BASE_PACKAGES: &[&str] = &["thunderbird"];

/// One tick box of the application step, in the order the cards are laid out.
///
/// The derived `Ord` **is** that layout order, which is what makes
/// [`AppSelection`]'s `BTreeMap` iterate in card order rather than
/// alphabetically — pinned by `card_ord_matches_all`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AppCard {
    /// The essentials, ticked by default: browser, mail, file manager,
    /// terminal, printing and the Flathub store.
    Base,
    /// Deliberately not named after one suite in the UI — the card enables the
    /// child the `programs.desktop` group declares in mxpkgs'
    /// `modules/index.json`, whichever that is.
    OfficeSuite,
    Modeling3D,
    /// Kdenlive plus DaVinci Resolve, whose edition the card's own radio row
    /// picks — see [`AppCard::variants`].
    VideoEditing,
    /// GIMP, Krita and Inkscape — raster retouching, painting and vector
    /// drawing in one card.
    Graphics,
    /// OBS Studio: screen and webcam recording as much as live streaming,
    /// which is what the card is named after.
    Streaming,
    /// Every launcher at once: Steam, Lutris, Heroic and UMU. Controller
    /// support comes with mxpkgs' `programs.games` group, which auto-enables
    /// from any of them.
    Gaming,
    /// The Unreal Engine 5 editor, which mxpkgs installs from an Epic archive
    /// into a Distrobox container of its own — see mxpkgs'
    /// `programs.dev.ue5`. Nothing is downloaded during the install: the
    /// container is created, and the archive asked for, on the launcher's
    /// first run on the installed system.
    GameDevelopment,
    /// Visual Studio Code, Git with the GitHub/GitLab tools, and Distrobox on
    /// a rootless Podman. The Ubuntu container is created by the launcher's
    /// first run on the installed system, never by the install itself — see
    /// mxpkgs' `programs.dev.distrobox`.
    Development,
}

impl AppCard {
    /// Grid order, read left to right then top to bottom: the essentials and
    /// the two "serious" cards first, then the creative row, then the gaming
    /// row, which the development card shares.
    pub const ALL: [AppCard; 9] = [
        AppCard::Base,
        AppCard::OfficeSuite,
        AppCard::Modeling3D,
        AppCard::VideoEditing,
        AppCard::Graphics,
        AppCard::Streaming,
        AppCard::Gaming,
        AppCard::GameDevelopment,
        AppCard::Development,
    ];

    /// nixpkgs packages this card adds to `package.nix`.
    ///
    /// # Parameters
    /// * `de` - desktop environment picked at step 8, which decides the
    ///   base pack's native file manager, terminal, viewers and editors.
    ///
    /// # Returns
    /// The attribute names, empty for every card but [`AppCard::Base`]: the
    /// optional cards' software comes in through mxpkgs modules, which can
    /// enable services and wire up hardware acceleration where a bare
    /// package list cannot.
    pub fn packages(self, de: DesktopEnvironment) -> Vec<&'static str> {
        match self {
            AppCard::Base => COMMON_BASE_PACKAGES
                .iter()
                .copied()
                .chain(de.base_pack_packages().iter().copied())
                .collect(),
            _ => Vec::new(),
        }
    }

    /// Modulix modules this card enables in `module.nix`, dotted as in
    /// mxpkgs' `modules/index.json` — one `mx.<name>.enable = true` each,
    /// written by `modulix_core_utils::init`.
    ///
    /// # Returns
    /// The module names; never empty, every card carries at least one. These
    /// are the **edition-independent** ones: a card offering several editions
    /// adds the picked one's modules on top, see [`AppCard::variants`].
    pub fn modules(self) -> &'static [&'static str] {
        match self {
            AppCard::Base => &[
                "programs.utilities.web-browser.firefox",
                "services.flatpak",
                "services.printer",
            ],
            AppCard::OfficeSuite => &["programs.desktop.onlyoffice"],
            AppCard::Modeling3D => &["programs.creativity.blender"],
            AppCard::VideoEditing => &["programs.studio.kdenlive"],
            AppCard::Graphics => &[
                "programs.creativity.gimp",
                "programs.creativity.krita",
                "programs.creativity.inkscape",
            ],
            AppCard::Streaming => &["programs.studio.obs-studio"],
            AppCard::Gaming => &[
                "programs.games.steam",
                "programs.games.lutris",
                "programs.games.heroic",
                "programs.games.umu",
            ],
            AppCard::GameDevelopment => &["programs.dev.ue5"],
            AppCard::Development => &[
                "programs.dev.vscode",
                "programs.dev.git",
                "programs.dev.distrobox",
            ],
        }
    }

    /// Mutually exclusive editions of the card's software, offered as a radio
    /// row inside the card itself (`steps::app_pack`).
    ///
    /// # Returns
    /// An empty slice for a card with a single edition — the common case — or
    /// at least two entries, index [`AppCard::DEFAULT_VARIANT`] being the one
    /// ticked when the page opens. Pinned by
    /// `variants_are_either_absent_or_a_real_choice`.
    pub fn variants(self) -> &'static [CardVariant] {
        match self {
            AppCard::VideoEditing => &[
                CardVariant {
                    key: "free",
                    modules: &["programs.studio.davinci-resolve"],
                },
                CardVariant {
                    key: "studio",
                    modules: &["programs.studio.davinci-resolve-studio"],
                },
            ],
            _ => &[],
        }
    }

    /// Index of the edition a card starts on: the first one declared.
    pub const DEFAULT_VARIANT: usize = 0;

    /// The edition `index` names, or `None` for a card that offers no choice.
    ///
    /// # Parameters
    /// * `index` - index into [`AppCard::variants`]; out of range falls back
    ///   to [`AppCard::DEFAULT_VARIANT`], so no caller can name an edition
    ///   that does not exist.
    pub fn variant_at(self, index: usize) -> Option<&'static CardVariant> {
        let variants = self.variants();
        variants
            .get(index)
            .or_else(|| variants.get(Self::DEFAULT_VARIANT))
    }
}

/// One mutually exclusive edition of an [`AppCard`] — a free versus a paid
/// build, a stable versus a bleeding-edge one.
///
/// Editions differ by **module**, never by package: the installer writes
/// `mx.<name>.enable = true` through `InitParams::modules` and has no way to
/// set an option's value, so a second edition is necessarily a second mxpkgs
/// module. That is also how mxpkgs models the two DaVinci Resolve builds —
/// the free module disables itself when the Studio one is enabled — so only
/// the picked one is ever written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CardVariant {
    /// Stable identifier, independent of the display language: the UI's label
    /// table and the tests key on it rather than on the position.
    pub key: &'static str,
    /// Modules this edition enables **on top of** [`AppCard::modules`]; never
    /// empty.
    pub modules: &'static [&'static str],
}

/// The application step's whole answer: which cards are ticked, and which
/// edition each of them is set to.
///
/// A `BTreeMap` keyed by [`AppCard`] so the selection is deduplicated and
/// iterated in card declaration order by construction, which makes the
/// package and module lists handed to `modulix_core_utils::init` byte-stable
/// across runs — they are committed into a git repository on the target. The
/// value is an index into [`AppCard::variants`], always
/// [`AppCard::DEFAULT_VARIANT`] for a card that offers no choice.
///
/// The empty selection is legal and means a bare system: no `package.nix`, no
/// `module.nix`, nothing beyond the desktop environment itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppSelection(BTreeMap<AppCard, usize>);

impl Default for AppSelection {
    /// Base pack only — every optional card starts unticked. This is what
    /// makes the base pack the answer even for an `InstallConfig` that never
    /// reached the application step.
    fn default() -> Self {
        Self::from_cards([AppCard::Base])
    }
}

impl AppSelection {
    /// Builds a selection out of the ticked cards, in any order and with
    /// duplicates allowed, each on its default edition.
    pub fn from_cards(cards: impl IntoIterator<Item = AppCard>) -> Self {
        Self::from_choices(
            cards
                .into_iter()
                .map(|card| (card, AppCard::DEFAULT_VARIANT)),
        )
    }

    /// Builds a selection out of the ticked cards and the edition picked for
    /// each.
    ///
    /// # Parameters
    /// * `choices` - `(card, index into card.variants())` pairs, in any order
    ///   and with duplicates allowed (the last one wins). An index out of
    ///   range is normalized to [`AppCard::DEFAULT_VARIANT`], so a stored
    ///   selection can never name an edition that does not exist.
    pub fn from_choices(choices: impl IntoIterator<Item = (AppCard, usize)>) -> Self {
        Self(
            choices
                .into_iter()
                .map(|(card, index)| {
                    let index = if index < card.variants().len() {
                        index
                    } else {
                        AppCard::DEFAULT_VARIANT
                    };
                    (card, index)
                })
                .collect(),
        )
    }

    /// Whether nothing is ticked, i.e. a bare system was asked for.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Whether one card is ticked.
    ///
    /// # Parameters
    /// * `card` - the card to look for.
    ///
    /// # Returns
    /// `true` when the user ticked it, whatever edition it is set to.
    pub fn has(&self, card: AppCard) -> bool {
        self.0.contains_key(&card)
    }

    /// The ticked cards, in [`AppCard::ALL`] order.
    pub fn cards(&self) -> impl Iterator<Item = AppCard> + '_ {
        self.0.keys().copied()
    }

    /// Index of the edition `card` is set to, [`AppCard::DEFAULT_VARIANT`]
    /// when it is unticked or offers no choice.
    pub fn variant(&self, card: AppCard) -> usize {
        self.0
            .get(&card)
            .copied()
            .unwrap_or(AppCard::DEFAULT_VARIANT)
    }

    /// The edition `card` is set to, or `None` when it offers no choice.
    pub fn variant_of(&self, card: AppCard) -> Option<&'static CardVariant> {
        card.variant_at(self.variant(card))
    }

    /// Union of every ticked card's packages.
    ///
    /// # Parameters
    /// * `de` - desktop environment, forwarded to [`AppCard::packages`].
    ///
    /// # Returns
    /// The attribute names, first occurrence winning, in card order rather
    /// than alphabetised — `package.nix` is read by a human afterwards and the
    /// base pack belongs at the top. Deduplicated with a linear scan: the
    /// lists are a dozen entries long.
    pub fn packages(&self, de: DesktopEnvironment) -> Vec<&'static str> {
        let mut out: Vec<&'static str> = Vec::new();
        for card in self.cards() {
            for package in card.packages(de) {
                if !out.contains(&package) {
                    out.push(package);
                }
            }
        }
        out
    }

    /// Union of every ticked card's modules, with the same ordering and
    /// deduplication contract as [`AppSelection::packages`].
    pub fn modules(&self) -> Vec<&'static str> {
        let mut out: Vec<&'static str> = Vec::new();
        for card in self.cards() {
            let edition = self.variant_of(card).map_or(&[][..], |v| v.modules);
            for module in card.modules().iter().chain(edition) {
                if !out.contains(module) {
                    out.push(module);
                }
            }
        }
        out
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
    fn only_the_base_card_ships_packages() {
        for de in DesktopEnvironment::ALL {
            assert!(!AppCard::Base.packages(de).is_empty());
            for card in AppCard::ALL {
                if card != AppCard::Base {
                    assert!(card.packages(de).is_empty(), "{card:?} ships packages");
                }
            }
        }
    }

    #[test]
    fn every_card_contributes_at_least_one_module() {
        for card in AppCard::ALL {
            assert!(!card.modules().is_empty(), "{card:?} enables no module");
        }
    }

    #[test]
    fn the_base_card_enables_flatpak_printing_and_firefox() {
        let base = AppCard::Base.modules();
        assert!(base.contains(&"services.flatpak"));
        assert!(base.contains(&"services.printer"));
        assert!(base.contains(&"programs.utilities.web-browser.firefox"));
    }

    /// The printing stack and Firefox both moved from the package list to an
    /// mxpkgs module; leaving the packages behind would install them twice and
    /// still never enable the service or the search provider.
    #[test]
    fn the_base_pack_no_longer_ships_module_provided_packages() {
        for de in DesktopEnvironment::ALL {
            let packages = AppCard::Base.packages(de);
            for stale in ["cups", "gutenprint", "system-config-printer", "firefox"] {
                assert!(!packages.contains(&stale), "{stale} is now module-provided");
            }
        }
    }

    #[test]
    fn the_gnome_base_pack_ships_disk_tools_and_the_tour() {
        let packages = AppCard::Base.packages(DesktopEnvironment::Gnome);
        assert!(packages.contains(&"gnome-disk-utility"));
        assert!(packages.contains(&"gnome-tour"));
        assert!(packages.contains(&"thunderbird"));
    }

    #[test]
    fn default_selection_is_base_only() {
        let selection = AppSelection::default();
        assert!(selection.cards().any(|c| c == AppCard::Base));
        for card in AppCard::ALL {
            if card != AppCard::Base {
                assert!(
                    !selection.cards().any(|c| c == card),
                    "{card:?} starts ticked"
                );
            }
        }
        assert_eq!(InstallConfig::default().apps, selection);
    }

    /// A radio row with a single entry is a UI dead end, and two editions
    /// sharing a key would make the label table and the tests ambiguous.
    #[test]
    fn variants_are_either_absent_or_a_real_choice() {
        for card in AppCard::ALL {
            let variants = card.variants();
            if variants.is_empty() {
                assert!(card.variant_at(0).is_none(), "{card:?} has no edition");
                continue;
            }
            assert!(variants.len() >= 2, "{card:?} offers a single edition");
            let mut keys: Vec<&str> = variants.iter().map(|v| v.key).collect();
            keys.sort_unstable();
            let count = keys.len();
            keys.dedup();
            assert_eq!(keys.len(), count, "{card:?} repeats an edition key");
            for variant in variants {
                assert!(
                    !variant.modules.is_empty(),
                    "{card:?}/{} enables no module",
                    variant.key
                );
            }
        }
    }

    /// The two DaVinci Resolve modules are mutually exclusive in mxpkgs, so
    /// exactly one of them may ever reach `module.nix`.
    #[test]
    fn the_video_editing_card_writes_one_resolve_edition() {
        let free = AppSelection::from_choices([(AppCard::VideoEditing, 0)]).modules();
        assert!(free.contains(&"programs.studio.kdenlive"));
        assert!(free.contains(&"programs.studio.davinci-resolve"));
        assert!(!free.contains(&"programs.studio.davinci-resolve-studio"));

        let studio = AppSelection::from_choices([(AppCard::VideoEditing, 1)]).modules();
        assert!(studio.contains(&"programs.studio.kdenlive"));
        assert!(studio.contains(&"programs.studio.davinci-resolve-studio"));
        assert!(!studio.contains(&"programs.studio.davinci-resolve"));
    }

    #[test]
    fn an_out_of_range_edition_falls_back_to_the_default() {
        let selection = AppSelection::from_choices([(AppCard::VideoEditing, 7)]);
        assert_eq!(selection.variant(AppCard::VideoEditing), 0);
        assert_eq!(
            selection.variant_of(AppCard::VideoEditing).map(|v| v.key),
            Some("free")
        );
        // A card with no edition of its own is normalized the same way.
        let base = AppSelection::from_choices([(AppCard::Base, 3)]);
        assert_eq!(base.variant(AppCard::Base), AppCard::DEFAULT_VARIANT);
        assert!(base.variant_of(AppCard::Base).is_none());
    }

    /// `AppSelection` leans on the derived `Ord` to iterate in layout order.
    #[test]
    fn card_ord_matches_all() {
        let sorted: Vec<AppCard> = AppSelection::from_cards(AppCard::ALL).cards().collect();
        assert_eq!(sorted, AppCard::ALL.to_vec());
    }

    #[test]
    fn an_empty_selection_installs_nothing() {
        let selection = AppSelection::from_cards([]);
        assert!(selection.is_empty());
        for de in DesktopEnvironment::ALL {
            assert!(selection.packages(de).is_empty());
        }
        assert!(selection.modules().is_empty());
    }

    #[test]
    fn the_union_is_deduplicated_in_card_order() {
        let selection = AppSelection::from_cards(AppCard::ALL);
        let modules = selection.modules();
        let mut seen = modules.clone();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), modules.len(), "duplicate module in the union");

        let flatpak = modules
            .iter()
            .position(|m| *m == "services.flatpak")
            .expect("the base card is ticked");
        let steam = modules
            .iter()
            .position(|m| *m == "programs.games.steam")
            .expect("the steam card is ticked");
        assert!(flatpak < steam, "the base card's modules must come first");
    }
}
