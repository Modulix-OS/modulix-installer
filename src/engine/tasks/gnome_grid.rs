use crate::config::{AppCard, AppSelection, DesktopEnvironment};
use crate::engine::tasks::{best_effort, report};
use crate::engine::{INSTALL_ROOT, ProgressEvent, ProgressSink, Task, TaskCtx};
use crate::mx;
use async_trait::async_trait;

/// The installed system's profile, as seen **from inside** the `nixos-enter`
/// chroot. `nixos-enter -- …` execs the command directly, with no PATH of its
/// own, so every binary has to be spelled out — same convention as
/// `SetPasswordsTask` and `FlatpakRemoteTask`.
const SYSTEM_PROFILE: &str = "/nix/var/nix/profiles/system/sw";

/// `dconf` in the target. Present whenever the desktop is GNOME: mxpkgs sets
/// `programs.dconf.enable = true`, and nixpkgs' module puts `pkgs.dconf` in
/// `environment.systemPackages`. Deliberately not taken from the live
/// session, whose PATH carries `glib.bin` (`gsettings`) and no `dconf` at all.
const DCONF_BIN: &str = "/nix/var/nix/profiles/system/sw/bin/dconf";

/// `chown` in the target, for the seeded `.config` tree.
const CHOWN_BIN: &str = "/nix/var/nix/profiles/system/sw/bin/chown";

/// Directory the generated keyfile is written to, chroot-relative.
/// `dconf compile` takes a *directory* of keyfiles, not a single file.
const SEED_DIR: &str = "/tmp/modulix-dconf-seed";

/// Candidate dash favourites, in dash order. Exactly the list mxpkgs'
/// `custom.nix` used to pin as a system default, minus the ones the target
/// turns out not to have: a `.desktop` that is not installed is filtered out
/// rather than trusted from the application selection, for the same reason
/// `FlatpakRemoteTask` keys off the binary existing — the file on disk is the
/// only reliable answer, and nothing in this repo maps a package attribute or
/// an mxpkgs module name to a `.desktop` id.
const FAVORITE_CANDIDATES: &[&str] = &[
    "firefox.desktop",
    "thunderbird.desktop",
    "org.gnome.Nautilus.desktop",
    "org.gnome.Software.desktop",
];

/// One app-grid folder.
///
/// Fields:
/// * `id` - dconf path component under `app-folders/folders/`, and the entry
///   in `folder-children`.
/// * `directory` - gnome-menus `.directory` file GNOME Shell reads the
///   localized folder name from, used with `translate=true`.
/// * `fallback_name` - English name written with `translate=false` when the
///   target ships no `desktop-directories` at all; without it GNOME would
///   display the raw string `Graphics.directory`.
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
    fallback_name: &'static str,
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
        fallback_name: "Office",
        categories: &["Office"],
        excluded_apps: &[],
        cards: &[AppCard::OfficeSuite],
    },
    GridFolder {
        id: "Graphics",
        directory: "Graphics.directory",
        fallback_name: "Graphics",
        categories: &["Graphics"],
        excluded_apps: &[],
        cards: &[AppCard::Graphics, AppCard::Modeling3D],
    },
    GridFolder {
        id: "AudioVideo",
        directory: "AudioVideo.directory",
        fallback_name: "Sound & Video",
        categories: &["AudioVideo", "Audio", "Video"],
        excluded_apps: &[],
        cards: &[AppCard::VideoEditing, AppCard::Streaming],
    },
    GridFolder {
        id: "Games",
        directory: "Game.directory",
        fallback_name: "Games",
        categories: &["Game"],
        excluded_apps: &[],
        cards: &[AppCard::Gaming],
    },
    GridFolder {
        id: "Development",
        directory: "Development.directory",
        fallback_name: "Development",
        categories: &["Development"],
        excluded_apps: &[],
        cards: &[AppCard::Development, AppCard::GameDevelopment],
    },
    GridFolder {
        id: "Utilities",
        directory: "Utility.directory",
        fallback_name: "Utilities",
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
        fallback_name: "System Tools",
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
    /// activation guarantee. Hence the slot before `PostInstallTask`.
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

        if tokio::fs::metadata(format!("{INSTALL_ROOT}{DCONF_BIN}"))
            .await
            .is_err()
        {
            report(
                tx,
                "GNOME app grid",
                Err("dconf is not installed in the target".to_string()),
            )
            .await;
            return Ok(());
        }

        // A kept `/home` partition can already hold this user's settings.
        // Seeding is a first-install courtesy, never a rewrite of what
        // somebody has tuned.
        let db_path = format!("{INSTALL_ROOT}/home/{username}/.config/dconf/user");
        if tokio::fs::metadata(&db_path).await.is_ok() {
            report(
                tx,
                "GNOME app grid",
                Err("the user already has a dconf database".to_string()),
            )
            .await;
            return Ok(());
        }

        let translate = has_desktop_directories().await;
        if !translate {
            let _ = tx
                .send(ProgressEvent::Log(
                    "GNOME app grid: the target ships no desktop-directories, \
                     folder names stay in English"
                        .to_string(),
                ))
                .await;
        }
        let keyfile = keyfile(
            &present_favorites().await,
            &wanted_folders(&ctx.config.apps),
            translate,
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
        let compiled = outcome.is_ok();
        report(tx, "GNOME app grid", outcome).await;

        if compiled {
            // A root-owned `.config` silently breaks every application on the
            // first login, so this is as load-bearing as the compile itself.
            let outcome = best_effort(
                "nixos-enter",
                &[
                    "--root",
                    INSTALL_ROOT,
                    "--",
                    CHOWN_BIN,
                    "-R",
                    &format!("{username}:users"),
                    &format!("/home/{username}/.config"),
                ],
            )
            .await;
            report(tx, "GNOME app grid ownership", outcome).await;
        }

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

/// Whether the target ships gnome-menus' `.directory` files, which is what
/// `translate=true` folder names are resolved against.
async fn has_desktop_directories() -> bool {
    tokio::fs::metadata(format!(
        "{INSTALL_ROOT}{SYSTEM_PROFILE}/share/desktop-directories"
    ))
    .await
    .is_ok()
}

/// The dash favourites to write: [`FAVORITE_CANDIDATES`], in that order,
/// restricted to the ones whose `.desktop` really is in the target's system
/// profile.
async fn present_favorites() -> Vec<&'static str> {
    let mut out = Vec::new();
    for id in FAVORITE_CANDIDATES {
        if tokio::fs::metadata(format!(
            "{INSTALL_ROOT}{SYSTEM_PROFILE}/share/applications/{id}"
        ))
        .await
        .is_ok()
        {
            out.push(*id);
        }
    }
    out
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
    let ticked: Vec<AppCard> = apps.cards().collect();
    FOLDERS
        .iter()
        .filter(|folder| {
            folder.cards.is_empty() || folder.cards.iter().any(|card| ticked.contains(card))
        })
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
/// * `favorites` - dash favourites, already filtered by [`present_favorites`].
/// * `folders` - folders to create, as [`wanted_folders`] chose them.
/// * `translate` - whether folder names may be left to GNOME Shell's
///   `.directory` lookup; `false` writes the English fallback names instead.
///
/// # Returns
/// The keyfile text. Groups are emitted in a fixed order so two installs
/// answering the same way produce the same bytes.
fn keyfile(favorites: &[&str], folders: &[&'static GridFolder], translate: bool) -> String {
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
        if translate {
            out.push_str(&format!("name='{}'\ntranslate=true\n", folder.directory));
        } else {
            out.push_str(&format!(
                "name='{}'\ntranslate=false\n",
                folder.fallback_name
            ));
        }
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
    fn keyfile_holds_both_halves() {
        let text = keyfile(
            &["firefox.desktop"],
            &wanted_folders(&AppSelection::from_cards([AppCard::Gaming])),
            true,
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
        let text = keyfile(&[], &wanted_folders(&AppSelection::default()), true);
        assert!(!text.contains("[org/gnome/shell]"));
        assert!(text.starts_with("[org/gnome/desktop/app-folders]"));
    }

    #[test]
    fn untranslated_names_fall_back_to_english() {
        let text = keyfile(&[], &wanted_folders(&AppSelection::from_cards([])), false);
        assert!(text.contains("name='Utilities'\ntranslate=false\n"));
        assert!(!text.contains(".directory"));
    }
}
