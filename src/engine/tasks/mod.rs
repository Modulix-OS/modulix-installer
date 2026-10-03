mod efi_entry;
mod encrypt;
mod enroll_tpm;
mod extra_config;
mod format;
mod init_config;
mod mount;
mod nixos_install;
mod partition;
mod post_install;
mod set_passwords;

pub use efi_entry::EfiEntryTask;
pub use encrypt::EncryptTask;
pub use enroll_tpm::EnrollTpmTask;
pub use extra_config::ExtraConfigTask;
pub use format::FormatTask;
pub use init_config::InitConfigTask;
pub use mount::MountTask;
pub use nixos_install::NixosInstallTask;
pub use partition::PartitionTask;
pub use post_install::PostInstallTask;
pub use set_passwords::SetPasswordsTask;

use crate::engine::{ProgressEvent, ProgressSink};

/// Runs a command that must not be able to fail the install, arguments as an
/// array (never a shell).
///
/// * `program` - command to run.
/// * `args` - its arguments.
///
/// # Returns
/// `Ok(())` on a zero exit, or a description of the failure — never an
/// `mx::Error`, so callers cannot accidentally fail the install with it.
pub(super) async fn best_effort(program: &str, args: &[&str]) -> Result<(), String> {
    capture(program, args).await.map(|_| ())
}

/// Same as [`best_effort`], but hands back what the command printed on
/// stdout.
///
/// * `program` - command to run.
/// * `args` - its arguments.
///
/// # Returns
/// The command's stdout on a zero exit, or a description of the failure —
/// never an `mx::Error`.
pub(super) async fn capture(program: &str, args: &[&str]) -> Result<String, String> {
    let output = tokio::process::Command::new(program)
        .args(args)
        .output()
        .await
        .map_err(|e| format!("{program} could not be run: {e}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Logs the outcome of one best-effort command.
///
/// * `tx` - progress sink.
/// * `what` - command name shown to the user.
/// * `outcome` - what [`best_effort`] returned.
pub(super) async fn report(tx: &ProgressSink, what: &str, outcome: Result<(), String>) {
    let line = match outcome {
        Ok(()) => format!("{what}: done"),
        Err(e) => format!("{what}: skipped ({e})"),
    };
    let _ = tx.send(ProgressEvent::Log(line)).await;
}
