use crate::backend::net::WifiProfile;
use crate::engine::tasks::report;
use crate::engine::{ProgressSink, Task, TaskCtx};
use crate::mx;
use async_trait::async_trait;
use std::path::Path;
use zeroize::Zeroizing;

/// NetworkManager's keyfile directory inside the install target. Same path
/// the installed system's own NetworkManager reads, created by nixpkgs'
/// module as a `systemd-tmpfiles` entry
/// (`d /etc/NetworkManager/system-connections 0700 root root`) — hence the
/// modes below, which match it rather than inventing their own.
const TARGET_DIR: &str = "/mnt/etc/NetworkManager/system-connections";

/// Mode of [`TARGET_DIR`], as nixpkgs' tmpfiles rule declares it.
const DIR_MODE: u32 = 0o700;

/// Mode of a copied keyfile. NetworkManager *refuses* to load a keyfile that
/// is readable by anyone else, and the file carries the network's pre-shared
/// key, so this is both a correctness and a confidentiality requirement.
const FILE_MODE: u32 = 0o600;

/// Copies the Wi-Fi profiles the wizard used onto the installed system, so
/// its first boot joins the same network without the user retyping the
/// password.
///
/// What is copied is NetworkManager's own keyfile, verbatim: the SSID, the
/// security type, the hidden flag and the pre-shared key are all already in
/// it, written by the daemon when `connect_wifi` called
/// `AddAndActivateConnection`. Re-deriving a profile from
/// `InstallConfig` would mean re-serialising secrets this installer has no
/// reason to touch again, and would lose whatever NetworkManager decided for
/// itself (`uuid`, `key-mgmt` details, the escaped file name).
///
/// A failed association deletes the profile it created
/// (`NetworkManagerBackend::discard_profile`), so only networks that really
/// worked during the install are carried over, and an ethernet or offline
/// install copies nothing.
///
/// Best-effort from end to end: the system is already on disk by the time
/// this runs and a missing profile only costs the user one Wi-Fi dialog on
/// the first boot, so neither an unreachable NetworkManager nor an I/O
/// failure may fail the install. Nothing about the file's *contents* is ever
/// logged — only the SSID and the destination name.
pub struct WifiProfileTask;

#[async_trait]
impl Task for WifiProfileTask {
    fn label(&self) -> String {
        "Copying the Wi-Fi configuration".to_string()
    }

    fn weight(&self) -> u32 {
        1
    }

    /// # Pre-conditions
    /// The target is mounted at `/mnt` (`MountTask` has run), which is why
    /// this stage precedes `PostInstallTask`.
    ///
    /// # Post-conditions
    /// Every Wi-Fi profile NetworkManager saved during the install is in
    /// `/mnt/etc/NetworkManager/system-connections`, `0600` and root-owned
    /// like the live copy, or the reason it is not is in the install log.
    ///
    /// # Returns
    /// Always `Ok(())` — see the type-level doc.
    async fn run(&self, ctx: &TaskCtx, tx: &ProgressSink) -> mx::Result<()> {
        let profiles = match ctx.backends.network.saved_wifi_profiles().await {
            Ok(profiles) => profiles,
            Err(e) => {
                report(tx, "Wi-Fi configuration", Err(e.to_string())).await;
                return Ok(());
            }
        };
        if profiles.is_empty() {
            report(
                tx,
                "Wi-Fi configuration",
                Err("no Wi-Fi network was used during the install".to_string()),
            )
            .await;
            return Ok(());
        }

        if let Err(e) = prepare_dir(TARGET_DIR).await {
            report(tx, "Wi-Fi configuration", Err(e)).await;
            return Ok(());
        }

        for profile in &profiles {
            let what = format!("Wi-Fi configuration ({})", profile.ssid);
            report(tx, &what, copy_profile(profile, TARGET_DIR).await).await;
        }
        Ok(())
    }
}

/// Creates the target keyfile directory and pins its mode.
///
/// * `dir` - host-side path of the directory, i.e. [`TARGET_DIR`].
///
/// # Post-conditions
/// `dir` exists and is mode [`DIR_MODE`], whether this call created it or
/// the install's own activation already had.
///
/// # Returns
/// `Ok(())`, or the I/O failure as a message — never an `mx::Error`, so a
/// caller cannot fail the install with it.
async fn prepare_dir(dir: &str) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;

    tokio::fs::create_dir_all(dir)
        .await
        .map_err(|e| format!("{dir} could not be created: {e}"))?;
    tokio::fs::set_permissions(dir, std::fs::Permissions::from_mode(DIR_MODE))
        .await
        .map_err(|e| format!("{dir} permissions could not be set: {e}"))
}

/// Copies one keyfile into the target directory.
///
/// * `profile` - the profile to copy, as the backend reported it.
/// * `dir` - host-side path of the target keyfile directory.
///
/// # Post-conditions
/// `dir/<file name of the source>` holds the same bytes as the source and is
/// mode [`FILE_MODE`] from the moment it exists — it is created with that
/// mode rather than `chmod`'d afterwards, so the pre-shared key it carries
/// is never briefly world-readable.
///
/// # Returns
/// `Ok(())`, or the first I/O failure as a message — never an `mx::Error`,
/// so a caller cannot fail the install with it.
async fn copy_profile(profile: &WifiProfile, dir: &str) -> Result<(), String> {
    use tokio::io::AsyncWriteExt;

    let source = profile.keyfile.as_path();
    // `file_name` drops every directory component, so a path from the
    // daemon cannot write outside `dir`.
    let name = file_name(source)?;
    // Zeroizing: the keyfile carries the network's pre-shared key in clear,
    // so this buffer is wiped when the copy is done rather than left in
    // freed heap memory for the rest of the session.
    let body = Zeroizing::new(
        tokio::fs::read(source)
            .await
            .map_err(|e| format!("{} could not be read: {e}", source.display()))?,
    );

    let target = format!("{dir}/{name}");
    let mut file = tokio::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(FILE_MODE)
        .open(&target)
        .await
        .map_err(|e| format!("{target} could not be created: {e}"))?;
    file.write_all(&body)
        .await
        .map_err(|e| format!("{target} could not be written: {e}"))?;
    file.flush()
        .await
        .map_err(|e| format!("{target} could not be written: {e}"))
}

/// Final component of a keyfile path.
///
/// * `source` - the path NetworkManager reported for the profile.
///
/// # Returns
/// The file name as a `&str`, or a message for a path that has none or is
/// not valid UTF-8 — both of which would mean a destination this task
/// cannot name.
fn file_name(source: &Path) -> Result<&str, String> {
    source
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("{} is not a usable keyfile name", source.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_name_keeps_only_the_last_component() {
        assert_eq!(
            file_name(Path::new(
                "/etc/NetworkManager/system-connections/Home.nmconnection"
            )),
            Ok("Home.nmconnection")
        );
    }

    #[test]
    fn file_name_strips_traversal() {
        assert_eq!(
            file_name(Path::new("/etc/NetworkManager/../shadow")),
            Ok("shadow")
        );
    }

    #[test]
    fn file_name_rejects_a_path_without_one() {
        assert!(file_name(Path::new("/")).is_err());
    }
}
