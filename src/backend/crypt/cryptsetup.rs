use super::CryptBackend;
use crate::mx;
use async_trait::async_trait;
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use tokio::io::AsyncWriteExt;

/// Rejects anything that is not a real device node.
///
/// `PipelineState` carries udisks2 object paths
/// (`/org/freedesktop/UDisks2/block_devices/vda2`), and handing one to
/// `cryptsetup` only earns an exit status with no explanation of what went
/// wrong. This turns the mistake into an error naming the offending argument,
/// at the one boundary where the two namespaces meet.
///
/// # Parameters
/// * `device` - the argument about to be passed to `cryptsetup` /
///   `systemd-cryptenroll`.
///
/// # Returns
/// `Ok(())` when `device` is an absolute path under `/dev/`.
///
/// # Errors
/// [`mx::Error::Backend`] naming `device` otherwise.
fn require_device_node(device: &str) -> mx::Result<()> {
    if device.starts_with("/dev/") {
        return Ok(());
    }
    Err(mx::Error::Backend(format!(
        "{device} is not a device node — a udisks2 object path must go through \
         DiskBackend::device_node before reaching cryptsetup"
    )))
}

async fn run_with_stdin(program: &str, args: &[&str], stdin_line: &str) -> mx::Result<()> {
    let mut child = tokio::process::Command::new(program)
        .args(args)
        .stdin(std::process::Stdio::piped())
        .spawn()?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(stdin_line.as_bytes()).await?;
        stdin.write_all(b"\n").await?;
    }
    let status = child.wait().await?;
    if !status.success() {
        return Err(mx::Error::Backend(format!(
            "{program} {args:?} exited with {status}"
        )));
    }
    Ok(())
}

/// A `0700` directory under `/run` holding the secrets handed to
/// `systemd-cryptenroll`, removed when the guard drops.
///
/// `/run` is a tmpfs, so nothing here ever reaches a disk — neither the
/// install target's nor the live medium's. Dropping is what guarantees the
/// cleanup: an early `?` on a failed enrollment must not leave a passphrase
/// behind for the rest of the session.
struct SecretDir {
    path: PathBuf,
}

impl SecretDir {
    /// Creates the directory with a name unique to this process.
    ///
    /// # Returns
    /// A guard owning the new directory.
    ///
    /// # Errors
    /// [`mx::Error`] from the underlying `mkdir`.
    fn new() -> mx::Result<Self> {
        let path = PathBuf::from(format!(
            "/run/modulixos-installer-cryptenroll.{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::DirBuilder::new().mode(0o700).create(&path)?;
        Ok(Self { path })
    }

    /// Writes one `0600` secret file into the directory.
    ///
    /// # Parameters
    /// * `name` - file name, used verbatim.
    /// * `secret` - content, written with no trailing newline:
    ///   `--unlock-key-file` takes the file as the full key, and a newline
    ///   would be part of it.
    ///
    /// # Returns
    /// The path of the file just written.
    ///
    /// # Errors
    /// [`mx::Error`] from creating or writing the file.
    fn write(&self, name: &str, secret: &str) -> mx::Result<PathBuf> {
        let path = self.path.join(name);
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)?;
        file.write_all(secret.as_bytes())?;
        Ok(path)
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for SecretDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

pub struct CryptsetupBackend;

impl CryptsetupBackend {
    pub fn new() -> Self {
        Self
    }
}

impl Default for CryptsetupBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl CryptBackend for CryptsetupBackend {
    async fn luks_format(&self, device: &str, passphrase: &str) -> mx::Result<()> {
        require_device_node(device)?;
        run_with_stdin(
            "cryptsetup",
            &["luksFormat", "--type", "luks2", "--batch-mode", device],
            passphrase,
        )
        .await
    }

    async fn luks_open(&self, device: &str, mapper_name: &str, passphrase: &str) -> mx::Result<()> {
        require_device_node(device)?;
        run_with_stdin("cryptsetup", &["open", device, mapper_name], passphrase).await
    }

    /// Both secrets travel through files in a tmpfs directory, never through
    /// argv (world-readable) nor stdin: `systemd-cryptenroll` asks for the
    /// existing passphrase through `ask_password`, which does not read the
    /// pipe it is given, so piping it only produced an enrollment that hung
    /// or failed. The unlock passphrase goes in via `--unlock-key-file`, and
    /// the new PIN — which has no command-line option at all — via the
    /// `cryptenroll.new-tpm2-pin` service credential.
    async fn enroll_tpm2(
        &self,
        device: &str,
        passphrase: &str,
        pin: Option<&str>,
    ) -> mx::Result<()> {
        require_device_node(device)?;

        let secrets = SecretDir::new()?;
        let key_file = secrets.write("unlock-key", passphrase)?;
        if let Some(pin) = pin {
            secrets.write("cryptenroll.new-tpm2-pin", pin)?;
        }

        let unlock_arg = format!("--unlock-key-file={}", key_file.display());
        let mut args = vec!["--tpm2-device=auto", unlock_arg.as_str()];
        if pin.is_some() {
            args.push("--tpm2-with-pin=yes");
        }
        args.push(device);

        let status = tokio::process::Command::new("systemd-cryptenroll")
            .args(&args)
            .env("CREDENTIALS_DIRECTORY", secrets.path())
            .status()
            .await?;
        if !status.success() {
            return Err(mx::Error::Backend(format!(
                "systemd-cryptenroll --tpm2-device=auto on {device} exited with {status}"
            )));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::require_device_node;

    #[test]
    fn accepts_a_device_node() {
        assert!(require_device_node("/dev/vda2").is_ok());
        assert!(require_device_node("/dev/mapper/modulixroot").is_ok());
    }

    #[test]
    fn rejects_a_udisks2_object_path() {
        let err = require_device_node("/org/freedesktop/UDisks2/block_devices/vda2")
            .expect_err("a udisks2 object path must be refused");
        assert!(err.to_string().contains("block_devices/vda2"));
        assert!(err.to_string().contains("device_node"));
    }

    #[test]
    fn rejects_a_relative_path() {
        assert!(require_device_node("vda2").is_err());
    }
}
