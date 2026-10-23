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

/// Candidate dash favourites, in dash order — exactly the list mxpkgs'
/// `custom.nix` used to pin as a system default — each paired with the
/// application card that brings it into the installed system, or `None` when
/// the GNOME choice alone does.
///
/// Firefox comes in as the base pack's `programs.utilities.web-browser.firefox`
/// module, Thunderbird and Nautilus as two of its packages
/// (`COMMON_BASE_PACKAGES`, `DesktopEnvironment::base_pack_packages`), and
/// GNOME Software from mxpkgs' own `mx.gnome.software` (default `true`, guarded
/// on `mx.desktop == "gnome"`), which is why only that one is unconditional.
///
/// Derived from the wizard's answers rather than from the files on disk: a
/// `.desktop` lives under the system profile, and that path cannot be probed
/// from the live ISO — see the type-level doc of [`GnomeGridTask`].
const FAVORITE_CANDIDATES: &[(&str, Option<AppCard>)] = &[
    ("firefox.desktop", Some(AppCard::Base)),
    ("thunderbird.desktop", Some(AppCard::Base)),
    ("org.gnome.Nautilus.desktop", Some(AppCard::Base)),
    ("org.gnome.Software.desktop", None),
];

/// One app-grid folder.
///
/// Fields:
/// * `id` - dconf path component under `app-folders/folders/`, and the entry
///   in `folder-children`.
/// * `directory` - gnome-menus `.directory` file GNOME Shell reads the
///   localized folder name from, used with `translate=true`. Always resolvable:
///   `gnome-menus` is one of the GNOME desktop module's packages and mxpkgs
///   does not exclude it, so choosing GNOME is what guarantees the
///   `desktop-directories` are there.
/// * `categories` - XDG categories the folder collects. Category-driven on
///   purpose: an application installed months later is filed by itself,
///   which an explicit `apps` list could never do.
/// * `excluded_apps` - ids kept on the root grid instead, for the two folders
///   broad enough to swallow the staples.
/// * `cards` - application cards that justify the folder; empty means
///   unconditional.
struct GridFolder {
    id: &'static str,
    directory: &'static str,
    categories: &'static [&'static str],
    excluded_apps: &'static [&'static str],
    cards: &'static [AppCard],
}

/// Taxonomy and exclusions revived from mxpkgs' removed `app-folders` block,
/// in `folder-children` order — which is also the order
/// [`wanted_folders`] returns, so the written key is byte-stable.
const FOLDERS: &[GridFolder] = &[
    GridFolder {
        id: "Office",
        directory: "Office.directory",
        categories: &["Office"],
        excluded_apps: &[],
        cards: &[AppCard::OfficeSuite],
    },
    GridFolder {
        id: "Graphics",
        directory: "Graphics.directory",
        categories: &["Graphics"],
        excluded_apps: &[],
        cards: &[AppCard::Graphics, AppCard::Modeling3D],
    },
    GridFolder {
        id: "AudioVideo",
        directory: "AudioVideo.directory",
        categories: &["AudioVideo", "Audio", "Video"],
        excluded_apps: &[],
        cards: &[AppCard::VideoEditing, AppCard::Streaming],
    },
    GridFolder {
        id: "Games",
        directory: "Game.directory",
        categories: &["Game"],
        excluded_apps: &[],
        cards: &[AppCard::Gaming],
    },
    GridFolder {
        id: "Development",
        directory: "Development.directory",
        categories: &["Development"],
        excluded_apps: &[],
        cards: &[AppCard::Development, AppCard::GameDevelopment],
    },
    GridFolder {
        id: "Utilities",
        directory: "Utility.directory",
        categories: &["Utility"],
        excluded_apps: &[
            "org.gnome.Nautilus.desktop",
            "org.gnome.Console.desktop",
            "org.gnome.TextEditor.desktop",
            "firefox.desktop",
            "thunderbird.desktop",
        ],
        cards: &[],
    },
    GridFolder {
        id: "System",
        directory: "System-Tools.directory",
        categories: &["System", "Settings"],
        excluded_apps: &["org.gnome.Settings.desktop"],
        cards: &[],
    },
];

/// Seeds the primary user's dconf database with an arranged GNOME app grid
/// and a pre-filled dash.
///
/// Written **into the user's own database**, once, rather than as a system
/// default: GNOME Shell deletes a folder or unpins an app by *resetting* the
/// key, and a reset falls back through to the default — which is why the
/// equivalent `programs.dconf.profiles.user.databases` block was dropped from
/// mxpkgs. Seeding the user db instead means every value here is the user's
/// own from the first login, so removing a folder or a favourite sticks.
///
/// Guarded on the wizard's answers, never on the files of the target: GNOME
/// being the chosen desktop is what puts `dconf` and gnome-menus'
/// `desktop-directories` in the installed system, and the base pack is what
/// puts each conditional dash favourite there. A host-side existence test
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
    /// `/home/<user>/.config/dconf/user` holds the grid folders and the dash
    /// favourites, owned by that user — or the reason it does not is in the
    /// install log.
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

        let keyfile = keyfile(
            &favorites(&ctx.config.apps),
            &wanted_folders(&ctx.config.apps),
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

/// The dash favourites to write.
///
/// * `apps` - the cards ticked at the application step.
///
/// # Returns
/// [`FAVORITE_CANDIDATES`], in that order, restricted to the ones the
/// selection really installs — an unconditional entry always, a conditional
/// one only while its card is ticked.
fn favorites(apps: &AppSelection) -> Vec<&'static str> {
    FAVORITE_CANDIDATES
        .iter()
        .filter(|(_, card)| card.is_none_or(|card| apps.has(card)))
        .map(|(id, _)| *id)
        .collect()
}

/// The folders worth creating for one application selection.
///
/// * `apps` - the cards ticked at the application step.
///
/// # Returns
/// The unconditional folders plus every folder at least one ticked card
/// justifies, in [`FOLDERS`] order. A folder nothing justifies is left out
/// rather than written empty — GNOME would hide it anyway, but an absent
/// folder is also one the user never has to delete.
fn wanted_folders(apps: &AppSelection) -> Vec<&'static GridFolder> {
    FOLDERS
        .iter()
        .filter(|folder| folder.cards.is_empty() || folder.cards.iter().any(|card| apps.has(*card)))
        .collect()
}

/// Renders a list of strings as a GVariant array, the only value shape the
/// written keys use.
fn gvariant_list(items: &[&str]) -> String {
    let inner = items
        .iter()
        .map(|item| format!("'{item}'"))
        .collect::<Vec<_>>()
        .join(", ");
    format!("[{inner}]")
}

/// Builds the dconf keyfile `dconf compile` turns into the user database.
///
/// * `favorites` - dash favourites, as [`favorites`] derived them.
/// * `folders` - folders to create, as [`wanted_folders`] chose them.
///
/// # Returns
/// The keyfile text. Groups are emitted in a fixed order so two installs
/// answering the same way produce the same bytes. Folder names are always
/// written with `translate=true`, so GNOME Shell localizes them itself — see
/// [`GridFolder`]'s `directory` field for why that lookup cannot fail.
fn keyfile(favorites: &[&str], folders: &[&'static GridFolder]) -> String {
    let mut out = String::new();
    if !favorites.is_empty() {
        out.push_str("[org/gnome/shell]\n");
        out.push_str(&format!("favorite-apps={}\n\n", gvariant_list(favorites)));
    }
    if folders.is_empty() {
        return out;
    }
    let ids = folders.iter().map(|f| f.id).collect::<Vec<_>>();
    out.push_str("[org/gnome/desktop/app-folders]\n");
    out.push_str(&format!("folder-children={}\n", gvariant_list(&ids)));
    for folder in folders {
        out.push_str(&format!(
            "\n[org/gnome/desktop/app-folders/folders/{}]\n",
            folder.id
        ));
        out.push_str(&format!("name='{}'\ntranslate=true\n", folder.directory));
        out.push_str(&format!(
            "categories={}\n",
            gvariant_list(folder.categories)
        ));
        if !folder.excluded_apps.is_empty() {
            out.push_str(&format!(
                "excluded-apps={}\n",
                gvariant_list(folder.excluded_apps)
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unconditional_folders_survive_an_empty_selection() {
        let folders = wanted_folders(&AppSelection::from_cards([]));
        let ids: Vec<&str> = folders.iter().map(|f| f.id).collect();
        assert_eq!(ids, vec!["Utilities", "System"]);
    }

    #[test]
    fn a_card_pulls_in_its_folder_in_table_order() {
        let folders = wanted_folders(&AppSelection::from_cards([
            AppCard::Gaming,
            AppCard::Modeling3D,
        ]));
        let ids: Vec<&str> = folders.iter().map(|f| f.id).collect();
        assert_eq!(ids, vec!["Graphics", "Games", "Utilities", "System"]);
    }

    #[test]
    fn every_card_is_covered_by_at_most_one_folder() {
        for card in AppCard::ALL {
            let hits = FOLDERS.iter().filter(|f| f.cards.contains(&card)).count();
            assert!(hits <= 1, "{card:?} is claimed by {hits} folders");
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
    fn keyfile_holds_both_halves() {
        let text = keyfile(
            &["firefox.desktop"],
            &wanted_folders(&AppSelection::from_cards([AppCard::Gaming])),
        );
        assert!(text.contains("favorite-apps=['firefox.desktop']"));
        assert!(text.contains("folder-children=['Games', 'Utilities', 'System']"));
        assert!(text.contains("[org/gnome/desktop/app-folders/folders/Games]"));
        assert!(text.contains("name='Game.directory'\ntranslate=true\n"));
        assert!(text.contains("categories=['Game']"));
        assert!(text.contains("excluded-apps=['org.gnome.Settings.desktop']"));
    }

    #[test]
    fn no_favorites_writes_no_shell_group() {
        let text = keyfile(&[], &wanted_folders(&AppSelection::default()));
        assert!(!text.contains("[org/gnome/shell]"));
        assert!(text.starts_with("[org/gnome/desktop/app-folders]"));
    }
}
