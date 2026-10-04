use crate::engine::{
    CONFIG_FLAKE_ATTR, CONFIG_REPO, INSTALL_ROOT, ProgressEvent, ProgressSink, Task, TaskCtx,
};
use crate::mx;
use async_trait::async_trait;
use std::collections::VecDeque;
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, BufReader};

/// How many of the last output lines are kept to describe a failure. Nix
/// traces and `builder for … failed` reports run well past twenty lines, and
/// this message is the only thing the user sees without opening the log.
const TAIL_LINES: usize = 200;

/// How many lines that look like an error are kept on top of the tail. Nix
/// usually states the real cause long before it stops talking, so the tail
/// alone can easily miss it.
const HIGHLIGHT_LINES: usize = 40;

/// Substrings a line is flagged on for [`HIGHLIGHT_LINES`]. Matched against
/// the lowercased line.
const ERROR_MARKERS: [&str; 4] = ["error", "failed", "cannot", "no space left"];

/// Builds and installs the target system with `nixos-install`.
///
/// This is the only stage that takes real time, and the reason the pipeline
/// does not go through modulix-core-utils' own rebuild: `rebuild_config`
/// inherits stdout and only captures stderr once the process has exited, so
/// it could never feed the install screen's log. Here both streams are piped
/// and forwarded line by line as `ProgressEvent::Log`.
pub struct NixosInstallTask;

#[async_trait]
impl Task for NixosInstallTask {
    fn label(&self) -> String {
        "Installing Modulix OS".to_string()
    }

    /// Dominates every other stage on purpose: partitioning and formatting
    /// take seconds, this takes minutes.
    fn weight(&self) -> u32 {
        200
    }

    /// # Pre-conditions
    /// `InitConfigTask` has committed the configuration repository, and the
    /// target root is mounted at `/mnt`.
    ///
    /// # Post-conditions
    /// The target system is built and its bootloader installed. No password
    /// is set yet — `SetPasswordsTask` handles that.
    async fn run(&self, _ctx: &TaskCtx, tx: &ProgressSink) -> mx::Result<()> {
        let flake = format!("{CONFIG_REPO}#{CONFIG_FLAKE_ATTR}");
        let _ = tx
            .send(ProgressEvent::Log(format!("nixos-install --flake {flake}")))
            .await;
        // nixos-install reports no progress fraction of its own, and this
        // single stage covers most of the wall-clock time.
        let _ = tx.send(ProgressEvent::Indeterminate(true)).await;

        let result = run_install(&flake, tx).await;

        let _ = tx.send(ProgressEvent::Indeterminate(false)).await;
        result
    }
}

/// Spawns `nixos-install` and streams its merged output into `tx`.
///
/// * `flake` - the flake reference to install, `<repo>#<attr>`.
/// * `tx` - progress sink every output line is forwarded to.
///
/// # Returns
/// `Ok(())` when `nixos-install` exits zero.
///
/// # Errors
/// `mx::Error::Backend` if the process cannot be spawned or exits non-zero,
/// carrying up to [`HIGHLIGHT_LINES`] lines that looked like errors plus the
/// last [`TAIL_LINES`] lines of output.
async fn run_install(flake: &str, tx: &ProgressSink) -> mx::Result<()> {
    // Arguments as an array, never a shell: this runs as root.
    let mut child = tokio::process::Command::new("nixos-install")
        .arg("--root")
        .arg(INSTALL_ROOT)
        .arg("--flake")
        .arg(flake)
        // Passwords are set afterwards with chpasswd so nothing plaintext
        // ends up in the Nix store.
        .arg("--no-root-password")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| mx::Error::Backend(format!("nixos-install could not be started: {e}")))?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| mx::Error::Backend("nixos-install gave no stdout".into()))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| mx::Error::Backend("nixos-install gave no stderr".into()))?;

    let (line_tx, line_rx) = async_channel::unbounded::<String>();
    let out_task = tokio::spawn(pump(BufReader::new(stdout), line_tx.clone()));
    let err_task = tokio::spawn(pump(BufReader::new(stderr), line_tx));

    let mut tail: VecDeque<String> = VecDeque::with_capacity(TAIL_LINES);
    let mut errors: VecDeque<String> = VecDeque::with_capacity(HIGHLIGHT_LINES);
    while let Ok(line) = line_rx.recv().await {
        if tail.len() == TAIL_LINES {
            tail.pop_front();
        }
        let lower = line.to_lowercase();
        if ERROR_MARKERS.iter().any(|m| lower.contains(m)) {
            if errors.len() == HIGHLIGHT_LINES {
                errors.pop_front();
            }
            errors.push_back(line.clone());
        }
        tail.push_back(line.clone());
        let _ = tx.send(ProgressEvent::Log(line)).await;
    }
    for (stream, task) in [("stdout", out_task), ("stderr", err_task)] {
        if let Err(e) = task.await {
            let _ = tx
                .send(ProgressEvent::Log(format!(
                    "[installer: the {stream} reader panicked: {e}]"
                )))
                .await;
        }
    }

    let status = child.wait().await?;
    if !status.success() {
        let mut report = format!("nixos-install exited with {status}");
        if !errors.is_empty() {
            report.push_str("\n\n--- reported errors ---\n");
            report.push_str(&Vec::from(errors).join("\n"));
        }
        report.push_str(&format!("\n\n--- last {} lines ---\n", tail.len()));
        report.push_str(&Vec::from(tail).join("\n"));
        return Err(mx::Error::Backend(report));
    }
    Ok(())
}

/// Forwards every line of one child stream into `sink` until EOF.
///
/// * `reader` - buffered child stdout or stderr.
/// * `sink` - channel both streams are merged into; dropped on return, which
///   is what ends the reader loop once every producer is gone.
///
/// # Post-conditions
/// Reads bytes, not `str`: `read_line` errors out on the first non-UTF-8 byte
/// without consuming it, which used to truncate the rest of the log silently.
/// Here the line is decoded lossily instead, and a genuine read error is
/// reported into `sink` rather than mimicking a clean EOF.
async fn pump<R>(mut reader: BufReader<R>, sink: async_channel::Sender<String>)
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut buf: Vec<u8> = Vec::new();
    loop {
        buf.clear();
        match reader.read_until(b'\n', &mut buf).await {
            Ok(0) => return,
            Ok(_) => {
                let line = String::from_utf8_lossy(&buf).trim_end().to_string();
                if sink.send(line).await.is_err() {
                    return;
                }
            }
            Err(e) => {
                let _ = sink
                    .send(format!("[installer: lost a child output stream: {e}]"))
                    .await;
                return;
            }
        }
    }
}
