use super::CryptBackend;
use crate::mx;
use async_trait::async_trait;
use tokio::io::AsyncWriteExt;

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
        run_with_stdin(
            "cryptsetup",
            &["luksFormat", "--type", "luks2", "--batch-mode", device],
            passphrase,
        )
        .await
    }

    async fn luks_open(&self, device: &str, mapper_name: &str, passphrase: &str) -> mx::Result<()> {
        run_with_stdin("cryptsetup", &["open", device, mapper_name], passphrase).await
    }

    async fn enroll_tpm2(&self, device: &str, pin: Option<&str>) -> mx::Result<()> {
        let mut args = vec!["--tpm2-device=auto"];
        if pin.is_some() {
            args.push("--tpm2-with-pin=yes");
        }
        args.push(device);
        // Best-effort: systemd-cryptenroll normally prompts interactively via
        // systemd-ask-password; piping the PIN on stdin works when it's not
        // attached to a controlling tty, but isn't guaranteed on every setup.
        run_with_stdin("systemd-cryptenroll", &args, pin.unwrap_or("")).await
    }
}
