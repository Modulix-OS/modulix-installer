use crate::config::{AppCard, AppSelection, DesktopEnvironment};
use crate::engine::tasks::{best_effort, report};
use crate::engine::{INSTALL_ROOT, ProgressSink, Task, TaskCtx};
use crate::mx;
use async_trait::async_trait;

/// `dconf` in the target, as seen **from inside** the `nixos-enter` chroot:
/// `nixos-enter -- …` execs the command directly, with no PATH of its own, so
/// every binary has to be spelled out — same convention as `SetPasswordsTask`
/// and `FlatpakRemoteTask`. Deliberately not taken from the live session,
/// whose PATH carries `glib.bin` (`gsettings`) and no `dconf` at all.
///
/// Present whenever the desktop is GNOME, which is what the task's first
/// guard checks: mxpkgs sets `programs.dconf.enable = true`, and nixpkgs'
/// module puts `pkgs.dconf` in `environment.systemPackages`.
const DCONF_BIN: &str = "/nix/var/nix/profiles/system/sw/bin/dconf";

/// `chown` in the target, for the seeded `.config` tree.
const CHOWN_BIN: &str = "/nix/var/nix/profiles/system/sw/bin/chown";

/// Directory the generated keyfile is written to, chroot-relative.
/// `dconf compile` takes a *directory* of keyfiles, not a single file.
const SEED_DIR: &str = "/tmp/modulix-dconf-seed";

/// Where one known application belongs on the grid.
enum Placement {
    /// On the root grid, in [`KNOWN_APPS`] order. Every folder whose
    /// categories would otherwise claim it excludes it.
    Root,
    /// Inside the folder of that [`GridFolder::id`], in [`KNOWN_APPS`] order.
    /// Every *other* folder whose categories would claim it excludes it.
    Folder(&'static str),
}

/// One application the installer knows the target will hold.
///
/// This table is the single source of truth behind the grid: it decides the
/// order inside each folder, the order of the root grid, which folders are
/// worth creating at all, and — through `categories` — the `excluded-apps`
/// that keep one application from being shown in two folders at once.
///
/// Fields:
/// * `id` - its `.desktop` id. An id this GNOME has no application for is
///   inert: `FolderView._loadApps` drops what `Shell.AppSystem.lookup_app`
///   cannot resolve, so a stale id only ever overcounts [`MIN_FOLDER_APPS`].
/// * `categories` - the XDG categories its `.desktop` really declares, which
///   is what [`excluded_apps`] matches folders against. An omitted category
///   means a duplicate this table fails to suppress; a surplus one only costs
///   an inert `excluded-apps` entry.
/// * `card` - the application card that installs it, `None` when choosing
///   GNOME alone does.
/// * `variant` - edition key (`CardVariant::key`) it is tied to, for a card
///   offering a choice; `None` when every edition brings it.
/// * `placement` - root grid or folder.
/// * `superseded_by` - card whose own applications replace this one. While
///   it is ticked the application is hidden from the grid entirely, through a
///   `Hidden=true` entry in the user.s `~/.local/share/applications` — the
///   XDG way of deleting one application at the user level, which takes
///   precedence over `XDG_DATA_DIRS` and which GLib honours by ignoring the
///   entry outright. Used for applications no package list controls because
///   the desktop itself ships them.
struct KnownApp {
    id: &'static str,
    categories: &'static [&'static str],
    card: Option<AppCard>,
    variant: Option<&'static str>,
    placement: Placement,
    superseded_by: Option<AppCard>,
}

/// Shorthand for an application kept on the root grid.
const fn root(
    id: &'static str,
    categories: &'static [&'static str],
    card: Option<AppCard>,
) -> KnownApp {
    KnownApp {
        id,
        categories,
        card,
        variant: None,
        placement: Placement::Root,
        superseded_by: None,
    }
}

/// Shorthand for an application filed in a folder by every edition of its
/// card.
const fn filed(
    id: &'static str,
    categories: &'static [&'static str],
    folder: &'static str,
    card: Option<AppCard>,
) -> KnownApp {
    KnownApp {
        id,
        categories,
        card,
        variant: None,
        placement: Placement::Folder(folder),
        superseded_by: None,
    }
}

/// Shorthand for an application only one edition of its card installs.
const fn filed_edition(
    id: &'static str,
    categories: &'static [&'static str],
    folder: &'static str,
    card: AppCard,
    variant: &'static str,
) -> KnownApp {
    KnownApp {
        id,
        categories,
        card: Some(card),
        variant: Some(variant),
        placement: Placement::Folder(folder),
        superseded_by: None,
    }
}

/// Shorthand for an application the desktop ships unconditionally and that
/// another card replaces — filed in a folder while that card is unticked,
/// hidden from the grid once it is.
const fn filed_superseded(
    id: &'static str,
    categories: &'static [&'static str],
    folder: &'static str,
    superseded_by: AppCard,
) -> KnownApp {
    KnownApp {
        id,
        categories,
        card: None,
        variant: None,
        placement: Placement::Folder(folder),
        superseded_by: Some(superseded_by),
    }
}

/// Every application the wizard can put on the target, root grid first — in
/// the order its icons are laid out — then each folder's contents in the
/// order they must appear inside it.
///
/// Derived from the wizard's answers rather than from the files on disk: a
/// `.desktop` lives under the system profile, and that path cannot be probed
/// from the live ISO — see the type-level doc of [`GnomeGridTask`].
const KNOWN_APPS: &[KnownApp] = &[
    // Root grid. Each of these matches at least one folder's categories
    // (Nautilus and the text editor are `Utility`, GNOME Software and the
    // console are `System`), and is kept out of it by `excluded_apps`.
    root(
        "firefox.desktop",
        &["Network", "WebBrowser"],
        Some(AppCard::Base),
    ),
    root(
        "org.gnome.Nautilus.desktop",
        &["Utility", "Core", "FileManager"],
        Some(AppCard::Base),
    ),
    root(
        "thunderbird.desktop",
        &["Network", "Chat", "Email", "Feed", "News"],
        Some(AppCard::Base),
    ),
    root(
        "org.gnome.Software.desktop",
        &["System", "PackageManager"],
        None,
    ),
    root(
        "org.gnome.TextEditor.desktop",
        &["Utility", "TextEditor"],
        Some(AppCard::Base),
    ),
    root(
        "org.gnome.Console.desktop",
        &["System", "TerminalEmulator", "Utility"],
        Some(AppCard::Base),
    ),
    // Office.
    filed(
        "onlyoffice-desktopeditors.desktop",
        &["Office", "WordProcessor", "Spreadsheet", "Presentation"],
        "Office",
        Some(AppCard::OfficeSuite),
    ),
    filed(
        "org.gnome.Papers.desktop",
        &[
            "Office",
            "Viewer",
            "Graphics",
            "2DGraphics",
            "VectorGraphics",
        ],
        "Office",
        Some(AppCard::Base),
    ),
    // Graphics.
    filed(
        "org.gnome.Loupe.desktop",
        &["Graphics", "2DGraphics", "RasterGraphics", "Viewer"],
        "Graphics",
        Some(AppCard::Base),
    ),
    filed(
        "gimp.desktop",
        &["Graphics", "2DGraphics", "RasterGraphics"],
        "Graphics",
        Some(AppCard::Graphics),
    ),
    filed(
        "krita.desktop",
        &["Graphics", "2DGraphics", "RasterGraphics"],
        "Graphics",
        Some(AppCard::Graphics),
    ),
    filed(
        "org.inkscape.Inkscape.desktop",
        &["Graphics", "VectorGraphics"],
        "Graphics",
        Some(AppCard::Graphics),
    ),
    filed(
        "blender.desktop",
        &["Graphics", "3DGraphics"],
        "Graphics",
        Some(AppCard::Modeling3D),
    ),
    // Audio and video.
    filed(
        "org.gnome.Showtime.desktop",
        &["AudioVideo", "Video", "Player"],
        "AudioVideo",
        Some(AppCard::Base),
    ),
    filed(
        "org.gnome.Decibels.desktop",
        &["AudioVideo", "Audio", "Music"],
        "AudioVideo",
        Some(AppCard::Base),
    ),
    filed(
        "org.kde.kdenlive.desktop",
        &["AudioVideo", "Video", "AudioVideoEditing"],
        "AudioVideo",
        Some(AppCard::VideoEditing),
    ),
    filed_edition(
        "davinci-resolve.desktop",
        &["AudioVideo", "AudioVideoEditing", "Video", "Graphics"],
        "AudioVideo",
        AppCard::VideoEditing,
        "free",
    ),
    filed_edition(
        "davinci-resolve-studio.desktop",
        &["AudioVideo", "AudioVideoEditing", "Video", "Graphics"],
        "AudioVideo",
        AppCard::VideoEditing,
        "studio",
    ),
    filed(
        "com.obsproject.Studio.desktop",
        &["AudioVideo", "Recorder"],
        "AudioVideo",
        Some(AppCard::Streaming),
    ),
    // Games. AdwSteamGtk declares `Utility` and no `Game` at all, so only
    // this table puts it next to Steam — and keeps it out of the utilities.
    filed(
        "steam.desktop",
        &["Network", "FileTransfer", "Game"],
        "Games",
        Some(AppCard::Gaming),
    ),
    filed(
        "net.lutris.Lutris.desktop",
        &["Game"],
        "Games",
        Some(AppCard::Gaming),
    ),
    filed(
        "com.heroicgameslauncher.hgl.desktop",
        &["Game"],
        "Games",
        Some(AppCard::Gaming),
    ),
    filed(
        "io.github.Foldex.AdwSteamGtk.desktop",
        &["Utility"],
        "Games",
        Some(AppCard::Gaming),
    ),
    // Development.
    filed(
        "code.desktop",
        &["Utility", "TextEditor", "Development", "IDE"],
        "Development",
        Some(AppCard::Development),
    ),
    filed(
        "mx-ue5.desktop",
        &["Development", "IDE"],
        "Development",
        Some(AppCard::GameDevelopment),
    ),
    filed(
        "mx-ubuntu-box.desktop",
        &["Development"],
        "Development",
        Some(AppCard::Development),
    ),
    // Utilities.
    filed(
        "org.gnome.Calculator.desktop",
        &["Utility", "Calculator"],
        "Utilities",
        Some(AppCard::Base),
    ),
    filed(
        "org.gnome.FileRoller.desktop",
        &["Utility", "Archiving", "Compression"],
        "Utilities",
        Some(AppCard::Base),
    ),
    // System tools.
    filed(
        "org.gnome.Settings.desktop",
        &["Settings", "DesktopSettings"],
        "System",
        None,
    ),
    filed(
        "org.gnome.DiskUtility.desktop",
        &["System", "Filesystem", "Settings"],
        "System",
        Some(AppCard::Base),
    ),
    filed(
        "net.nokyan.Resources.desktop",
        &["System", "Monitor"],
        "System",
        Some(AppCard::Base),
    ),
    filed(
        "org.gnome.baobab.desktop",
        &["System", "Filesystem", "Utility"],
        "System",
        Some(AppCard::Base),
    ),
    // GNOME ships the Extensions app with the shell itself, so no package
    // list can take it out; Extension Manager does everything it does and
    // installs from extensions.gnome.org on top, so the base pack hides it.
    filed_superseded(
        "org.gnome.Extensions.desktop",
        &["Utility"],
        "System",
        AppCard::Base,
    ),
    filed(
        "com.mattjakeman.ExtensionManager.desktop",
        &["Utility"],
        "System",
        Some(AppCard::Base),
    ),
];

/// Dash favourites, in dash order — exactly the list mxpkgs' `custom.nix`
/// used to pin as a system default. Each id is a [`KNOWN_APPS`] entry kept on
/// the root grid (pinned by `the_dash_is_pinned_to_root_applications`), so
/// what installs it is read from that table rather than repeated here:
/// Firefox, Thunderbird and Nautilus come with the base pack, GNOME Software
/// with mxpkgs' own `mx.gnome.software` (default `true`, guarded on
/// `mx.desktop == "gnome"`), which is why it is pinned unconditionally.
const DASH_ORDER: &[&str] = &[
    "firefox.desktop",
    "thunderbird.desktop",
    "org.gnome.Nautilus.desktop",
    "org.gnome.Software.desktop",
];

/// A folder is only worth *showing* from two applications up: a folder holding
/// a single icon hides it behind one more click for nothing. Below the
/// threshold the folder is still written, empty — see [`FolderPlan::shown`].
const MIN_FOLDER_APPS: usize = 2;

/// One app-grid folder.
///
/// Fields:
/// * `id` - dconf path component under `app-folders/folders/`, the entry in
///   `folder-children`, and what [`KnownApp`]'s `placement` names.
/// * `directory` - gnome-menus `.directory` file GNOME Shell reads the
///   localized folder name from, used with `translate=true`. Always resolvable:
///   `gnome-menus` is one of the GNOME desktop module's packages and mxpkgs
///   does not exclude it, so choosing GNOME is what guarantees the
///   `desktop-directories` are there.
/// * `categories` - XDG categories the folder collects. Still category-driven
///   on purpose: an application installed months later is filed by itself,
///   which a `KNOWN_APPS` list alone could never do.
struct GridFolder {
    id: &'static str,
    directory: &'static str,
    categories: &'static [&'static str],
}

/// The folders, in `folder-children` order — which is also the order
/// [`folder_plans`] returns, so the written key is byte-stable.
const FOLDERS: &[GridFolder] = &[
    GridFolder {
        id: "Office",
        directory: "Office.directory",
        categories: &["Office"],
    },
    GridFolder {
        id: "Graphics",
        directory: "Graphics.directory",
        categories: &["Graphics"],
    },
    GridFolder {
        id: "AudioVideo",
        directory: "AudioVideo.directory",
        categories: &["AudioVideo", "Audio", "Video"],
    },
    GridFolder {
        id: "Games",
        directory: "Game.directory",
        categories: &["Game"],
    },
    GridFolder {
        id: "Development",
        directory: "Development.directory",
        categories: &["Development"],
    },
    GridFolder {
        id: "Utilities",
        directory: "Utility.directory",
        categories: &["Utility"],
    },
    GridFolder {
        id: "System",
        directory: "System-Tools.directory",
        categories: &["System", "Settings"],
    },
];

/// One folder to write, with the members the selection really installs — what
/// [`folder_plans`] returns and [`keyfile`] renders.
///
/// Fields:
/// * `folder` - the folder itself, one of [`FOLDERS`].
/// * `members` - the known applications this install really puts in it, in
///   [`KNOWN_APPS`] order.
struct FolderPlan {
    folder: &'static GridFolder,
    members: Vec<&'static str>,
}

impl FolderPlan {
    /// Whether the folder is worth *showing* now.
    ///
    /// Every folder is written whatever this returns: `categories=` is the
    /// only thing that can file an application installed months later, and
    /// this seed runs once, at install time. Below [`MIN_FOLDER_APPS`] the
    /// folder is written *empty* instead — no `apps` key, its own members in
    /// `excluded-apps`, its id out of `app-picker-layout` — and GNOME Shell
    /// destroys a folder icon holding nothing (`AppDisplay._loadApps`), so the
    /// first login looks exactly as it did when such a folder was not written
    /// at all.
    ///
    /// # Returns
    /// `true` from [`MIN_FOLDER_APPS`] members up.
    fn shown(&self) -> bool {
        self.members.len() >= MIN_FOLDER_APPS
    }
}
/// Seeds the primary user's dconf database with an arranged GNOME app grid
/// and a pre-filled dash.
///
/// Written **into the user's own database**, once, rather than as a system
/// default: GNOME Shell deletes a folder, unpins an app or moves an icon by
/// *writing or resetting* the key, and a reset falls back through to the
/// default — which is why the equivalent `programs.dconf.profiles.user.databases`
/// block was dropped from mxpkgs. Seeding the user db instead means every
/// value here is the user's own from the first login, so removing a folder,
/// a favourite or dragging an icon elsewhere sticks — the root-grid order
/// below is a starting point, not a policy.
///
/// Guarded on the wizard's answers, never on the files of the target: GNOME
/// being the chosen desktop is what puts `dconf` and gnome-menus'
/// `desktop-directories` in the installed system, and the application cards
/// are what put each listed application there. A host-side existence test
/// could not answer either question — `nixos-install` builds into the
/// target's own store, so `/mnt/nix/var/nix/profiles/system` is an absolute
/// symlink into a `/nix/store` path the live ISO does not have, and probing
/// it reads as "not installed" whatever the truth. Only the two paths that
/// really live on the target (`/mnt/home/<user>`, and the dconf database
/// inside it) are tested on disk.
///
/// Best-effort from end to end: the system is already on disk by the time
/// this runs, and a grid that is not arranged is a cosmetic loss, never a
/// reason to fail an install.
pub struct GnomeGridTask;

#[async_trait]
impl Task for GnomeGridTask {
    fn label(&self) -> String {
        "Arranging the application grid".to_string()
    }

    fn weight(&self) -> u32 {
        1
    }

    /// # Pre-conditions
    /// `NixosInstallTask` has completed, the target is still mounted at
    /// `/mnt` — including `/mnt/home` when `/home` is a separate partition —
    /// and the primary user's home already exists, which
    /// `modulix_core_utils`' `createHome = true` plus the install's own
    /// activation guarantee (and which the third guard checks rather than
    /// assumes). Hence the slot before `PostInstallTask`.
    ///
    /// # Post-conditions
    /// `/home/<user>/.config/dconf/user` holds the grid folders, the root
    /// layout and the dash favourites, and
    /// `/home/<user>/.local/share/applications` holds one `Hidden=true` entry
    /// per superseded application, both owned by that user — or the reason
    /// they do not is in the install log.
    ///
    /// # Returns
    /// Always `Ok(())` — see the type-level doc.
    async fn run(&self, ctx: &TaskCtx, tx: &ProgressSink) -> mx::Result<()> {
        if ctx.config.desktop_environment != DesktopEnvironment::Gnome {
            report(
                tx,
                "GNOME app grid",
                Err("the desktop environment is not GNOME".to_string()),
            )
            .await;
            return Ok(());
        }

        let username = ctx.config.user.username.trim().to_string();
        if username.is_empty() {
            report(
                tx,
                "GNOME app grid",
                Err("no primary user to seed".to_string()),
            )
            .await;
            return Ok(());
        }

        // The home is the one thing this stage cannot create itself: seeding
        // into a missing one would leave `/mnt/home/<user>` owned by root,
        // which breaks every application on the first login.
        let home = format!("{INSTALL_ROOT}/home/{username}");
        if tokio::fs::metadata(&home).await.is_err() {
            report(tx, "GNOME app grid", Err(format!("{home} does not exist"))).await;
            return Ok(());
        }

        // A kept `/home` partition can already hold this user's settings.
        // Seeding is a first-install courtesy, never a rewrite of what
        // somebody has tuned.
        let db_path = format!("{home}/.config/dconf/user");
        if tokio::fs::metadata(&db_path).await.is_ok() {
            report(
                tx,
                "GNOME app grid",
                Err("the user already has a dconf database".to_string()),
            )
            .await;
            return Ok(());
        }

        let folders = folder_plans(&ctx.config.apps);
        let keyfile = keyfile(
            &favorites(&ctx.config.apps),
            &root_layout(&ctx.config.apps, &folders),
            &folders,
        );

        let seed_dir = format!("{INSTALL_ROOT}{SEED_DIR}");
        let outcome = write_seed(&seed_dir, &keyfile, &db_path).await;
        let outcome = match outcome {
            Ok(()) => {
                best_effort(
                    "nixos-enter",
                    &[
                        "--root",
                        INSTALL_ROOT,
                        "--",
                        DCONF_BIN,
                        "compile",
                        &format!("/home/{username}/.config/dconf/user"),
                        SEED_DIR,
                    ],
                )
                .await
            }
            Err(e) => Err(e),
        };
        report(tx, "GNOME app grid", outcome).await;

        // Applications the desktop ships and a ticked card replaces. Written
        // at the user level, which is the only layer that can hide one: the
        // shell's own `.desktop` files are in the system profile, and
        // `environment.gnome.excludePackages` cannot reach an application
        // gnome-shell itself carries.
        let hidden = hidden_entries(&ctx.config.apps);
        if !hidden.is_empty() {
            let applications = format!("{home}/.local/share/applications");
            let outcome = write_hidden_entries(&applications, &hidden).await;
            report(tx, "GNOME hidden applications", outcome).await;

            // Same reasoning as the `.config` chown below, and unconditional
            // for the same reason: `write_hidden_entries` has created the
            // tree by the time a failure is reported.
            let outcome = best_effort(
                "nixos-enter",
                &[
                    "--root",
                    INSTALL_ROOT,
                    "--",
                    CHOWN_BIN,
                    "-R",
                    &format!("{username}:"),
                    &format!("/home/{username}/.local"),
                ],
            )
            .await;
            report(tx, "GNOME hidden applications ownership", outcome).await;
        }

        // Unconditional, and as load-bearing as the compile itself: a
        // root-owned `.config` silently breaks every application on the first
        // login, and `write_seed` has already created it by the time a failed
        // compile is reported. `<user>:` takes the user's login group rather
        // than assuming `users`.
        let outcome = best_effort(
            "nixos-enter",
            &[
                "--root",
                INSTALL_ROOT,
                "--",
                CHOWN_BIN,
                "-R",
                &format!("{username}:"),
                &format!("/home/{username}/.config"),
            ],
        )
        .await;
        report(tx, "GNOME app grid ownership", outcome).await;

        let _ = tokio::fs::remove_dir_all(&seed_dir).await;
        Ok(())
    }
}

/// Creates the seed directory and the user's `dconf` directory, then writes
/// the keyfile.
///
/// * `seed_dir` - host-side path of the keyfile directory `dconf compile`
///   reads.
/// * `keyfile` - its contents, as [`keyfile`] built them.
/// * `db_path` - host-side path of the database to be written, whose parent
///   directory has to exist before `dconf compile` runs.
///
/// # Returns
/// `Ok(())` once both directories exist and the keyfile is on disk, or the
/// first I/O error as a message — never an `mx::Error`, so a caller cannot
/// fail the install with it.
async fn write_seed(seed_dir: &str, keyfile: &str, db_path: &str) -> Result<(), String> {
    tokio::fs::create_dir_all(seed_dir)
        .await
        .map_err(|e| format!("{seed_dir} could not be created: {e}"))?;
    let parent = std::path::Path::new(db_path)
        .parent()
        .ok_or_else(|| format!("{db_path} has no parent directory"))?
        .to_path_buf();
    tokio::fs::create_dir_all(&parent)
        .await
        .map_err(|e| format!("{} could not be created: {e}", parent.display()))?;
    let path = format!("{seed_dir}/apps");
    tokio::fs::write(&path, keyfile)
        .await
        .map_err(|e| format!("{path} could not be written: {e}"))
}

/// Whether a selection really installs one known application.
///
/// * `app` - the [`KNOWN_APPS`] entry to test.
/// * `apps` - the cards ticked at the application step.
fn is_installed(app: &KnownApp, apps: &AppSelection) -> bool {
    let Some(card) = app.card else {
        return true;
    };
    if !apps.has(card) {
        return false;
    }
    let Some(key) = app.variant else {
        return true;
    };
    card.variant_at(apps.variant(card))
        .is_some_and(|picked| picked.key == key)
}

/// Writes one `Hidden=true` desktop entry per superseded application.
///
/// * `applications` - host-side path of the user's
///   `~/.local/share/applications`, created if missing.
/// * `entries` - the applications to hide, as [`hidden_entries`] chose them.
///
/// # Returns
/// `Ok(())` once every file is on disk, or the first I/O error as a message
/// — never an `mx::Error`, so a caller cannot fail the install with it.
///
/// `Hidden=true` is the XDG spelling of "the user deleted this entry": GLib
/// drops such a file *and* every lower-precedence one with the same id, so
/// the shell's copy under the system profile disappears from the grid.
/// `NoDisplay=true` says the same thing to anything that only implements
/// that half. Deleting the file restores the application, which is what
/// makes this reversible after the install.
async fn write_hidden_entries(
    applications: &str,
    entries: &[&'static KnownApp],
) -> Result<(), String> {
    tokio::fs::create_dir_all(applications)
        .await
        .map_err(|e| format!("{applications} could not be created: {e}"))?;
    for entry in entries {
        let name = entry
            .id
            .strip_suffix(".desktop")
            .unwrap_or(entry.id)
            .rsplit('.')
            .next()
            .unwrap_or(entry.id);
        let path = format!("{applications}/{}", entry.id);
        let body = format!(
            "[Desktop Entry]\nType=Application\nName={name}\nNoDisplay=true\nHidden=true\n"
        );
        tokio::fs::write(&path, body)
            .await
            .map_err(|e| format!("{path} could not be written: {e}"))?;
    }
    Ok(())
}

/// Whether a selection hides one known application from the grid.
///
/// * `app` - the [`KNOWN_APPS`] entry to test.
/// * `apps` - the cards ticked at the application step.
///
/// # Returns
/// `true` once the card that supersedes it is ticked, which is what makes
/// [`hidden_entries`] write a `Hidden=true` desktop entry for it.
fn is_superseded(app: &KnownApp, apps: &AppSelection) -> bool {
    app.superseded_by.is_some_and(|card| apps.has(card))
}

/// Whether a selection installs one known application *and* shows it.
fn is_visible(app: &KnownApp, apps: &AppSelection) -> bool {
    is_installed(app, apps) && !is_superseded(app, apps)
}

/// The applications to hide from the grid, in [`KNOWN_APPS`] order.
///
/// * `apps` - the cards ticked at the application step.
fn hidden_entries(apps: &AppSelection) -> Vec<&'static KnownApp> {
    KNOWN_APPS
        .iter()
        .filter(|app| is_installed(app, apps) && is_superseded(app, apps))
        .collect()
}

/// The [`KNOWN_APPS`] entry one `.desktop` id names, if any.
fn known(id: &str) -> Option<&'static KnownApp> {
    KNOWN_APPS.iter().find(|app| app.id == id)
}

/// Whether two category lists have at least one entry in common.
fn categories_intersect(left: &[&str], right: &[&str]) -> bool {
    left.iter().any(|category| right.contains(category))
}

/// The dash favourites to write.
///
/// * `apps` - the cards ticked at the application step.
///
/// # Returns
/// [`DASH_ORDER`], in that order, restricted to the ones the selection really
/// installs — an unconditional entry always, a conditional one only while its
/// card is ticked.
fn favorites(apps: &AppSelection) -> Vec<&'static str> {
    DASH_ORDER
        .iter()
        .filter_map(|id| known(id))
        .filter(|app| is_installed(app, apps))
        .map(|app| app.id)
        .collect()
}

/// The members of one folder the selection really installs.
///
/// * `folder` - the folder to fill.
/// * `apps` - the cards ticked at the application step.
///
/// # Returns
/// Their ids in [`KNOWN_APPS`] order, which is the order they are shown in:
/// `FolderView._compareItems` sorts by index in the written `apps` key before
/// falling back to alphabetical order.
fn members(folder: &GridFolder, apps: &AppSelection) -> Vec<&'static str> {
    KNOWN_APPS
        .iter()
        .filter(|app| matches!(app.placement, Placement::Folder(id) if id == folder.id))
        .filter(|app| is_visible(app, apps))
        .map(|app| app.id)
        .collect()
}

/// The `excluded-apps` key of one folder.
///
/// * `folder` - the folder to compute it for.
/// * `orphans` - applications filed in this very folder that must stay on the
///   root grid, which is how a folder below [`MIN_FOLDER_APPS`] is kept empty.
///   Pass `&[]` for a folder that is shown. It must hold the folder's
///   **installed** members only, never its whole potential membership from
///   [`KNOWN_APPS`]: excluding an application the install does not contain yet
///   would blacklist it from the folder for good, which is the very thing
///   writing every folder exists to avoid.
///
/// # Returns
/// Every known application this folder's categories would claim although it
/// belongs elsewhere, in [`KNOWN_APPS`] order. This is what keeps one
/// application out of two folders at once — Papers is `Office` *and*
/// `Graphics`, DaVinci Resolve `AudioVideo` *and* `Graphics`, VS Code
/// `Development` *and* `Utility`, Baobab `System` *and* `Utility` — and what
/// keeps the root-grid staples on the root grid: GNOME Software and the
/// console are `System`, Nautilus and the text editor are `Utility`. The
/// settings and the extensions apps are the opposite case — `Settings` and
/// `Utility` respectively, and filed in `System` on purpose, which is what
/// takes them out of the utilities and off the root grid.
///
/// Excluding an application the install does not contain costs nothing, so the
/// list is the same whatever the user ticked — except for `orphans`, the one
/// part that does follow the answers. Two installs answering the same way are
/// still byte-identical.
fn excluded_apps(folder: &GridFolder, orphans: &[&str]) -> Vec<&'static str> {
    KNOWN_APPS
        .iter()
        .filter(|app| {
            !matches!(app.placement, Placement::Folder(id) if id == folder.id)
                || orphans.contains(&app.id)
        })
        .filter(|app| categories_intersect(app.categories, folder.categories))
        .map(|app| app.id)
        .collect()
}

/// The folders to write for one application selection.
///
/// * `apps` - the cards ticked at the application step.
///
/// # Returns
/// One plan per [`FOLDERS`] entry, in table order — so `folder-children` is
/// byte-stable — each with the applications this install really puts in it.
/// Every folder is written: a folder below [`MIN_FOLDER_APPS`] is the one an
/// application installed months later needs most, and only
/// [`FolderPlan::shown`] decides whether it holds anything at the first login.
fn folder_plans(apps: &AppSelection) -> Vec<FolderPlan> {
    FOLDERS
        .iter()
        .map(|folder| FolderPlan {
            folder,
            members: members(folder, apps),
        })
        .collect()
}

/// The root grid, in the order its icons are placed on the first page.
///
/// * `apps` - the cards ticked at the application step.
/// * `folders` - every folder, as [`folder_plans`] built them.
///
/// # Returns
/// The installed root-grid entries of [`KNOWN_APPS`], then the applications
/// whose folder stays below [`MIN_FOLDER_APPS`] — a folder nobody can see is
/// no place for them — then the shown folder ids, then, left to GNOME Shell
/// which appends them itself, everything else the target holds. One
/// [`KNOWN_APPS`] pass yields the first two groups in table order, the root
/// entries coming first in the table.
fn root_layout(apps: &AppSelection, folders: &[FolderPlan]) -> Vec<&'static str> {
    let orphaned = |folder_id: &str| {
        folders
            .iter()
            .any(|plan| plan.folder.id == folder_id && !plan.shown())
    };
    KNOWN_APPS
        .iter()
        .filter(|app| is_visible(app, apps))
        .filter(|app| match app.placement {
            Placement::Root => true,
            Placement::Folder(id) => orphaned(id),
        })
        .map(|app| app.id)
        .chain(
            folders
                .iter()
                .filter(|plan| plan.shown())
                .map(|plan| plan.folder.id),
        )
        .collect()
}

/// Renders a list of strings as a GVariant array, the value shape every
/// string-list key uses.
fn gvariant_list(items: &[&str]) -> String {
    let inner = items
        .iter()
        .map(|item| format!("'{item}'"))
        .collect::<Vec<_>>()
        .join(", ");
    format!("[{inner}]")
}

/// Renders an ordered id list as the `aa{sv}` value `app-picker-layout`
/// holds: one page, each id mapped to its position.
///
/// The spelling matches the key's own default in
/// `org.gnome.shell.gschema.xml`, so `dconf compile` — which knows no schema
/// and parses the text on its own — infers the type GNOME Shell reads back.
fn gvariant_layout(items: &[&str]) -> String {
    let inner = items
        .iter()
        .enumerate()
        .map(|(position, id)| format!("'{id}': <{{'position': <{position}>}}>"))
        .collect::<Vec<_>>()
        .join(", ");
    format!("[{{{inner}}}]")
}

/// Builds the dconf keyfile `dconf compile` turns into the user database.
///
/// * `favorites` - dash favourites, as [`favorites`] derived them.
/// * `layout` - root-grid order, as [`root_layout`] derived it.
/// * `folders` - every folder and its contents, as [`folder_plans`] built
///   them.
///
/// # Returns
/// The keyfile text. Groups are emitted in a fixed order so two installs
/// answering the same way produce the same bytes. Every folder gets
/// `categories` (what keeps filing applications nobody listed) and
/// `excluded-apps` (what keeps it from claiming an application filed
/// elsewhere); only a [`FolderPlan::shown`] folder also gets `apps`, its
/// order. A folder below the threshold is therefore written and empty — its
/// own members are in its `excluded-apps` — which is how it can file a later
/// install without showing a single icon behind one click today. The key is
/// left unset rather than written as `apps=[]`: `get_strv` answers `[]`
/// either way, and GNOME Shell writes the key itself on the first drag.
/// Folder names are always written with `translate=true`, so GNOME Shell
/// localizes them itself — see [`GridFolder`]'s `directory` field for why that
/// lookup cannot fail.
fn keyfile(favorites: &[&str], layout: &[&str], folders: &[FolderPlan]) -> String {
    let mut out = String::new();
    if !favorites.is_empty() || !layout.is_empty() {
        out.push_str("[org/gnome/shell]\n");
        if !favorites.is_empty() {
            out.push_str(&format!("favorite-apps={}\n", gvariant_list(favorites)));
        }
        if !layout.is_empty() {
            out.push_str(&format!("app-picker-layout={}\n", gvariant_layout(layout)));
        }
        out.push('\n');
    }
    if folders.is_empty() {
        return out;
    }
    let ids = folders
        .iter()
        .map(|plan| plan.folder.id)
        .collect::<Vec<_>>();
    out.push_str("[org/gnome/desktop/app-folders]\n");
    out.push_str(&format!("folder-children={}\n", gvariant_list(&ids)));
    for plan in folders {
        let folder = plan.folder;
        out.push_str(&format!(
            "\n[org/gnome/desktop/app-folders/folders/{}]\n",
            folder.id
        ));
        out.push_str(&format!("name='{}'\ntranslate=true\n", folder.directory));
        if plan.shown() {
            out.push_str(&format!("apps={}\n", gvariant_list(&plan.members)));
        }
        out.push_str(&format!(
            "categories={}\n",
            gvariant_list(folder.categories)
        ));
        let orphans: &[&str] = if plan.shown() { &[] } else { &plan.members };
        let excluded = excluded_apps(folder, orphans);
        if !excluded.is_empty() {
            out.push_str(&format!("excluded-apps={}\n", gvariant_list(&excluded)));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The folders a selection really *shows*, in table order — every folder
    /// is written now, so this is what the grid looks like at the first login.
    fn folder_ids(apps: &AppSelection) -> Vec<&'static str> {
        folder_plans(apps)
            .iter()
            .filter(|plan| plan.shown())
            .map(|plan| plan.folder.id)
            .collect()
    }

    /// What GNOME Shell would really show in `folder` for one known
    /// application: its `apps` key, or its categories minus `excluded-apps`.
    fn folder_shows(folder: &GridFolder, app: &KnownApp) -> bool {
        if matches!(app.placement, Placement::Folder(id) if id == folder.id) {
            return true;
        }
        categories_intersect(app.categories, folder.categories)
            && !excluded_apps(folder, &[]).contains(&app.id)
    }

    #[test]
    fn no_application_is_shown_in_two_folders() {
        for app in KNOWN_APPS {
            let folders: Vec<&str> = FOLDERS
                .iter()
                .filter(|folder| folder_shows(folder, app))
                .map(|folder| folder.id)
                .collect();
            assert!(folders.len() <= 1, "{} would show in {folders:?}", app.id);
        }
    }

    #[test]
    fn root_applications_are_never_swallowed_by_a_folder() {
        for app in KNOWN_APPS {
            if !matches!(app.placement, Placement::Root) {
                continue;
            }
            for folder in FOLDERS {
                assert!(
                    !folder_shows(folder, app),
                    "{} would leave the root grid for {}",
                    app.id,
                    folder.id
                );
            }
        }
    }

    #[test]
    fn no_id_is_listed_twice() {
        let mut ids: Vec<&str> = KNOWN_APPS.iter().map(|app| app.id).collect();
        let count = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), count, "an application is listed twice");
    }

    #[test]
    fn every_placement_names_a_real_folder() {
        for app in KNOWN_APPS {
            let Placement::Folder(id) = app.placement else {
                continue;
            };
            assert!(
                FOLDERS.iter().any(|folder| folder.id == id),
                "{} is filed in the unknown folder {id}",
                app.id
            );
        }
    }

    #[test]
    fn gnome_alone_only_fills_the_system_folder() {
        // The settings and the extensions app come with GNOME itself, so
        // they reach the threshold whatever the user ticked.
        assert_eq!(folder_ids(&AppSelection::from_cards([])), vec!["System"]);
    }

    #[test]
    fn the_base_pack_fills_three_folders() {
        // Showtime + Decibels, Calculator + File Roller, Disks + Resources +
        // Baobab. Papers and Loupe are alone in theirs, so they stay on the
        // root grid instead.
        assert_eq!(
            folder_ids(&AppSelection::from_cards([AppCard::Base])),
            vec!["AudioVideo", "Utilities", "System"]
        );
    }

    #[test]
    fn a_single_application_is_left_on_the_root_grid() {
        // Blender alone does not justify *showing* the Graphics folder; GIMP,
        // Krita and Inkscape do. The folder is written either way.
        assert!(
            !folder_ids(&AppSelection::from_cards([AppCard::Modeling3D])).contains(&"Graphics")
        );
        assert!(folder_ids(&AppSelection::from_cards([AppCard::Graphics])).contains(&"Graphics"));
    }

    #[test]
    fn the_office_card_is_what_makes_the_office_folder_reach_two() {
        let base = AppSelection::from_cards([AppCard::Base]);
        assert!(!folder_ids(&base).contains(&"Office"));
        let with_suite = AppSelection::from_cards([AppCard::Base, AppCard::OfficeSuite]);
        assert_eq!(folder_ids(&with_suite)[0], "Office");
    }

    #[test]
    fn a_card_pulls_in_its_folder_in_table_order() {
        let ids = folder_ids(&AppSelection::from_cards([
            AppCard::Gaming,
            AppCard::Graphics,
        ]));
        assert_eq!(ids, vec!["Graphics", "Games", "System"]);
    }

    #[test]
    fn only_the_picked_resolve_edition_is_listed() {
        let folder = FOLDERS
            .iter()
            .find(|folder| folder.id == "AudioVideo")
            .expect("the multimedia folder exists");

        let free = AppSelection::from_choices([(AppCard::VideoEditing, 0)]);
        let listed = members(folder, &free);
        assert!(listed.contains(&"davinci-resolve.desktop"));
        assert!(!listed.contains(&"davinci-resolve-studio.desktop"));

        let studio = AppSelection::from_choices([(AppCard::VideoEditing, 1)]);
        let listed = members(folder, &studio);
        assert!(listed.contains(&"davinci-resolve-studio.desktop"));
        assert!(!listed.contains(&"davinci-resolve.desktop"));
    }

    #[test]
    fn steam_leads_the_games_folder() {
        let apps = AppSelection::from_cards([AppCard::Base, AppCard::Gaming]);
        let folders = folder_plans(&apps);
        let games = folders
            .iter()
            .find(|plan| plan.folder.id == "Games")
            .expect("the gaming card fills the folder");
        assert_eq!(games.members[0], "steam.desktop");
        let text = keyfile(&[], &[], &folders);
        assert!(text.contains("apps=['steam.desktop', 'net.lutris.Lutris.desktop'"));
    }

    #[test]
    fn the_base_pack_hides_the_extensions_app() {
        let base = AppSelection::from_cards([AppCard::Base]);
        let hidden: Vec<&str> = hidden_entries(&base).iter().map(|app| app.id).collect();
        assert_eq!(hidden, vec!["org.gnome.Extensions.desktop"]);

        let system = FOLDERS
            .iter()
            .find(|folder| folder.id == "System")
            .expect("the system folder exists");
        let listed = members(system, &base);
        assert!(!listed.contains(&"org.gnome.Extensions.desktop"));
        assert!(listed.contains(&"com.mattjakeman.ExtensionManager.desktop"));
    }

    #[test]
    fn without_the_base_pack_the_extensions_app_stays() {
        let bare = AppSelection::from_cards([]);
        assert!(hidden_entries(&bare).is_empty());
        let system = FOLDERS
            .iter()
            .find(|folder| folder.id == "System")
            .expect("the system folder exists");
        assert!(members(system, &bare).contains(&"org.gnome.Extensions.desktop"));
    }

    #[test]
    fn the_dash_is_pinned_to_root_applications() {
        for id in DASH_ORDER {
            let app = known(id).unwrap_or_else(|| panic!("{id} is not a known application"));
            assert!(
                matches!(app.placement, Placement::Root),
                "{id} is pinned to the dash but filed in a folder"
            );
        }
    }

    #[test]
    fn the_base_pack_brings_every_favorite() {
        let all = favorites(&AppSelection::from_cards([AppCard::Base]));
        assert_eq!(
            all,
            vec![
                "firefox.desktop",
                "thunderbird.desktop",
                "org.gnome.Nautilus.desktop",
                "org.gnome.Software.desktop",
            ]
        );
    }

    #[test]
    fn without_the_base_pack_only_gnome_software_is_pinned() {
        let bare = favorites(&AppSelection::from_cards([AppCard::Gaming]));
        assert_eq!(bare, vec!["org.gnome.Software.desktop"]);
    }

    #[test]
    fn the_root_layout_puts_the_applications_before_the_folders() {
        let apps = AppSelection::from_cards([AppCard::Base, AppCard::Gaming]);
        let folders = folder_plans(&apps);
        let layout = root_layout(&apps, &folders);
        assert_eq!(layout[0], "firefox.desktop");
        assert_eq!(layout.last(), Some(&"System"));

        // Papers and Loupe are orphans of two folders the base pack leaves
        // below the threshold, so they sit between the root staples and the
        // folders — but every application still comes before every folder.
        let is_folder = |id: &str| FOLDERS.iter().any(|folder| folder.id == id);
        let first_folder = layout
            .iter()
            .position(|id| is_folder(id))
            .expect("the layout ends with the shown folders");
        assert!(layout[..first_folder].iter().all(|id| !is_folder(id)));
        assert!(layout[first_folder..].iter().all(|id| is_folder(id)));
        assert_eq!(
            layout[..first_folder]
                .iter()
                .rev()
                .take(2)
                .collect::<Vec<_>>(),
            vec![&"org.gnome.Loupe.desktop", &"org.gnome.Papers.desktop"]
        );
    }

    #[test]
    fn the_root_layout_skips_what_is_not_installed() {
        let bare = AppSelection::from_cards([]);
        assert_eq!(
            root_layout(&bare, &folder_plans(&bare)),
            vec!["org.gnome.Software.desktop", "System"]
        );
    }

    #[test]
    fn keyfile_holds_every_half() {
        let apps = AppSelection::from_cards([AppCard::Base, AppCard::Gaming]);
        let folders = folder_plans(&apps);
        let text = keyfile(&favorites(&apps), &root_layout(&apps, &folders), &folders);
        assert!(text.contains("favorite-apps=['firefox.desktop'"));
        assert!(text.contains("app-picker-layout=[{'firefox.desktop': <{'position': <0>}>,"));
        assert!(text.contains(
            "folder-children=['Office', 'Graphics', 'AudioVideo', 'Games', \
             'Development', 'Utilities', 'System']"
        ));
        assert!(text.contains("[org/gnome/desktop/app-folders/folders/Games]"));
        assert!(text.contains("name='Game.directory'\ntranslate=true\n"));
        assert!(text.contains("categories=['Game']"));
        assert!(text.contains("excluded-apps=['org.gnome.Software.desktop'"));
    }

    #[test]
    fn nothing_selected_writes_the_root_layout_and_the_system_folder() {
        let bare = AppSelection::from_cards([]);
        let folders = folder_plans(&bare);
        let text = keyfile(&favorites(&bare), &root_layout(&bare, &folders), &folders);
        assert!(text.starts_with("[org/gnome/shell]"));
        assert!(text.contains(
            "folder-children=['Office', 'Graphics', 'AudioVideo', 'Games', \
             'Development', 'Utilities', 'System']"
        ));
        assert!(
            text.contains("apps=['org.gnome.Settings.desktop', 'org.gnome.Extensions.desktop']")
        );
    }

    /// One folder group of a keyfile, from just after its header to the next
    /// group — what GNOME really reads for that folder.
    fn folder_group<'a>(text: &'a str, id: &str) -> &'a str {
        let header = format!("[org/gnome/desktop/app-folders/folders/{id}]\n");
        let start = text.find(&header).expect("the folder is written") + header.len();
        let rest = &text[start..];
        match rest.find("\n[") {
            Some(end) => &rest[..end],
            None => rest,
        }
    }

    /// The selections every "whatever the selection" test runs over.
    fn selection_matrix() -> Vec<AppSelection> {
        vec![
            AppSelection::from_cards([]),
            AppSelection::from_cards([AppCard::Base]),
            AppSelection::from_cards([AppCard::Base, AppCard::Modeling3D]),
            AppSelection::from_cards([AppCard::Base, AppCard::Gaming, AppCard::Graphics]),
        ]
    }

    #[test]
    fn every_folder_is_written_whatever_the_selection() {
        for apps in selection_matrix() {
            let plans = folder_plans(&apps);
            assert_eq!(plans.len(), FOLDERS.len());
            let text = keyfile(&[], &root_layout(&apps, &plans), &plans);
            for folder in FOLDERS {
                assert!(
                    text.contains(&format!(
                        "[org/gnome/desktop/app-folders/folders/{}]",
                        folder.id
                    )),
                    "{} is not written",
                    folder.id
                );
            }
        }
    }

    #[test]
    fn a_folder_below_the_threshold_is_written_but_empty() {
        let modeling = AppSelection::from_cards([AppCard::Modeling3D]);
        let plans = folder_plans(&modeling);
        let graphics = plans
            .iter()
            .find(|plan| plan.folder.id == "Graphics")
            .expect("every folder is planned");
        assert!(!graphics.shown());

        let group = keyfile(&[], &[], &plans);
        let group = folder_group(&group, "Graphics");
        assert!(group.contains("categories=['Graphics']"));
        // `excluded-apps=` ends in the same three characters, so the key has
        // to be matched at the start of its line.
        assert!(!group.contains("\napps="), "the folder claims {group}");
        assert!(group.contains("'blender.desktop'"));
    }

    #[test]
    fn a_folder_below_the_threshold_keeps_its_member_on_the_root_grid() {
        let modeling = AppSelection::from_cards([AppCard::Modeling3D]);
        let plans = folder_plans(&modeling);
        let layout = root_layout(&modeling, &plans);
        assert!(layout.contains(&"blender.desktop"));
        assert!(!layout.contains(&"Graphics"));

        let text = keyfile(&[], &layout, &plans);
        assert!(text.contains("'blender.desktop': <{'position'"));
        assert!(!text.contains("'Graphics': <{'position'"));
    }

    #[test]
    fn a_folder_below_the_threshold_still_claims_a_later_install() {
        // The whole point of writing a folder nobody can see yet: Blender
        // alone leaves Graphics hidden, but GIMP installed months later is
        // still filed by the folder's categories. Pre-excluding a member the
        // install does not contain is what would blacklist it for good.
        let modeling = AppSelection::from_cards([AppCard::Modeling3D]);
        let graphics = FOLDERS
            .iter()
            .find(|folder| folder.id == "Graphics")
            .expect("the graphics folder exists");
        let excluded = excluded_apps(graphics, &members(graphics, &modeling));
        assert!(excluded.contains(&"blender.desktop"));
        assert!(!excluded.contains(&"gimp.desktop"));
        assert!(!excluded.contains(&"krita.desktop"));
    }

    #[test]
    fn a_suppressed_folder_claims_none_of_its_installed_members() {
        for apps in selection_matrix() {
            for plan in folder_plans(&apps) {
                if plan.shown() {
                    continue;
                }
                let excluded = excluded_apps(plan.folder, &plan.members);
                for id in &plan.members {
                    assert!(
                        excluded.contains(id),
                        "{id} would make the hidden folder {} appear",
                        plan.folder.id
                    );
                }
            }
        }
    }
}
