use crate::engine::{INSTALL_ROOT, ProgressEvent, ProgressSink, Task, TaskCtx};
use crate::mx;
use async_trait::async_trait;
use std::process::Stdio;
use tokio::io::AsyncWriteExt;

/// Sets the primary user's and root's password on the freshly installed
/// system.
///
/// Deliberately done outside the NixOS configuration: `users.users.<name>`
/// only offers `initialPassword` (plaintext, world-readable in the Nix store)
/// or `hashedPassword`, and modulix-core-utils writes the former with an
/// empty value. Piping into `chpasswd` under `nixos-enter` writes
/// `/etc/shadow` directly instead, so no password — hashed or not — ever
/// reaches the store, and nothing appears on a command line either.
///
/// This relies on `users.mutableUsers` staying at its default `true`: with
/// declarative users the written `/etc/shadow` would be overwritten on the
/// first activation.
pub struct SetPasswordsTask;

#[async_trait]
impl Task for SetPasswordsTask {
    fn label(&self) -> String {
        "Setting passwords".to_string()
    }

    fn weight(&self) -> u32 {
        2
    }

    /// # Pre-conditions
    /// `NixosInstallTask` has completed, so `/mnt` holds a usable system.
    ///
    /// # Post-conditions
    /// Both accounts have the password collected by the user step. Nothing
    /// is logged beyond the account names.
    async fn run(&self, ctx: &TaskCtx, tx: &ProgressSink) -> mx::Result<()> {
        let username = ctx.config.user.username.trim();
        let password = &ctx.config.user.password;
        if username.is_empty() {
            return Err(mx::Error::Backend("no username was collected".into()));
        }
        if password.is_empty() {
            return Err(mx::Error::Backend("no password was collected".into()));
        }
        // chpasswd reads `name:password` lines, so neither may contain one.
        if password.contains('\n') || password.contains(':') {
            return Err(mx::Error::Backend(
                "the password cannot contain a colon or a line break".into(),
            ));
        }

        let batch = format!("root:{password}\n{username}:{password}\n");
        chpasswd(&batch).await?;

        let _ = tx
            .send(ProgressEvent::Log(format!(
                "passwords set for root and {username}"
            )))
            .await;
        Ok(())
    }
}

/// Runs `chpasswd` inside the installed system with `batch` on its stdin.
///
/// * `batch` - `name:password` lines, newline-terminated.
///
/// # Returns
/// `Ok(())` once `chpasswd` exited successfully.
///
/// # Errors
/// `mx::Error::Backend` if `nixos-enter` cannot be spawned or the command
/// exits non-zero. The error message never carries `batch`.
async fn chpasswd(batch: &str) -> mx::Result<()> {
    let mut child = tokio::process::Command::new("nixos-enter")
        .arg("--root")
        .arg(INSTALL_ROOT)
        .arg("--")
        // Absolute path inside the chroot: `nixos-enter -- …` execs the command
        // directly, with no PATH of its own. This is the same profile path
        // `nixos-enter` uses for its own default shell.
        .arg("/nix/var/nix/profiles/system/sw/bin/chpasswd")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| mx::Error::Backend(format!("nixos-enter could not be started: {e}")))?;

    {
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| mx::Error::Backend("nixos-enter gave no stdin".into()))?;
        stdin.write_all(batch.as_bytes()).await?;
        stdin.shutdown().await?;
    }

    let output = child.wait_with_output().await?;
    if !output.status.success() {
        return Err(mx::Error::Backend(format!(
            "chpasswd failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(())
}
