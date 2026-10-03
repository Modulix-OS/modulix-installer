//! Persistent install log.
//!
//! The install progress page keeps its log in a `gtk::TextBuffer`, which dies
//! with the process. That is not enough to diagnose a failed install: the live
//! ISO root is a RAM overlay and journald on the ISO is volatile, so **nothing
//! written in the live session survives a reboot**. This module therefore
//! mirrors every log line to three places:
//!
//! * [`LIVE_LOG`] — readable for the rest of the live session (tty2 is still a
//!   getty; only tty1 is taken over by the kiosk).
//! * `stderr` — the kiosk session runs under systemd, so this reaches journald
//!   and `journalctl -b`. Volatile too, but available without leaving the app.
//! * [`TARGET_LOG`] — on the freshly installed disk, the **only** copy that
//!   survives a reboot. Written by [`copy_to_target`], which refuses to write
//!   unless [`crate::engine::INSTALL_ROOT`] really is a mount point.

use crate::engine::{INSTALL_ROOT, ProgressEvent};
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::sync::OnceLock;
use tokio::io::AsyncWriteExt;

/// Log file inside the live session. On the ISO `/var/log` is a writable
/// tmpfs; it is where `cat`/`less` from a rescue TTY looks first.
pub const LIVE_LOG: &str = "/var/log/modulixos-install.log";

/// Used when [`LIVE_LOG`] cannot be created (read-only `/var`, dev machine
/// without root). `/tmp` is writable in every environment the installer runs
/// in.
const LIVE_LOG_FALLBACK: &str = "/tmp/modulixos-install.log";

/// Copy of the log on the installed system — the only one that survives a
/// reboot of the target machine.
pub const TARGET_LOG: &str = "/mnt/var/log/modulixos-install.log";

static LIVE_PATH: OnceLock<Option<&'static str>> = OnceLock::new();

/// Path the live log is being written to, resolved once per process.
///
/// Tries [`LIVE_LOG`] first, then [`LIVE_LOG_FALLBACK`]; each is truncated on
/// first use so a second install attempt in the same session does not append
/// to the previous one.
///
/// # Post-conditions
/// The returned path, when `Some`, names an existing file the process can
/// append to. Resolution happens at most once; later calls return the same
/// value without touching the filesystem.
///
/// # Returns
/// `Some(path)` if either candidate could be created, `None` if neither could
/// — in which case only `stderr` carries the log.
pub fn path() -> Option<&'static str> {
    *LIVE_PATH.get_or_init(|| {
        [LIVE_LOG, LIVE_LOG_FALLBACK]
            .into_iter()
            .find(|candidate| std::fs::File::create(candidate).is_ok())
    })
}

/// Appends [`ProgressEvent`]s to the live log and to `stderr`.
///
/// Held by the tee task that sits between the pipeline and the UI, so writes
/// never happen on the GTK main loop.
pub struct Writer {
    /// `None` when [`path`] found nowhere writable; `stderr` still gets
    /// every line.
    file: Option<tokio::fs::File>,
    /// Monotonic origin for the per-line timestamps. Deliberately not a wall
    /// clock: no extra dependency, and no locale-dependent formatting.
    started: std::time::Instant,
}

impl Writer {
    /// Opens the live log for appending.
    ///
    /// # Post-conditions
    /// Never fails: a writer whose file could not be opened degrades to
    /// `stderr` only.
    pub async fn open() -> Self {
        let file = match path() {
            Some(p) => tokio::fs::OpenOptions::new()
                .append(true)
                .open(p)
                .await
                .ok(),
            None => None,
        };
        Self {
            file,
            started: std::time::Instant::now(),
        }
    }

    /// Records one pipeline event.
    ///
    /// * `ev` - event to record. [`ProgressEvent::Progress`] and
    ///   [`ProgressEvent::Indeterminate`] carry no text and are skipped.
    ///
    /// # Post-conditions
    /// The line reached `stderr`. It also reached the live log unless no file
    /// could be opened or the write failed — a failing log must never abort
    /// an install, so write errors are dropped.
    pub async fn record(&mut self, ev: &ProgressEvent) {
        let line = match ev {
            ProgressEvent::Started { task } => format!("==> {task}"),
            ProgressEvent::Finished { task } => format!("<== {task}"),
            ProgressEvent::Log(l) => l.clone(),
            ProgressEvent::Progress { .. } | ProgressEvent::Indeterminate(_) => return,
        };
        eprintln!("[install] {line}");
        if let Some(file) = &mut self.file {
            let stamped = format!("[{:>8.2}s] {line}\n", self.started.elapsed().as_secs_f64());
            let _ = file.write_all(stamped.as_bytes()).await;
        }
    }

    /// Flushes the live log.
    ///
    /// # Post-conditions
    /// Everything [`Writer::record`] accepted is on disk, as far as the
    /// filesystem is concerned.
    pub async fn flush(&mut self) {
        if let Some(file) = &mut self.file {
            let _ = file.flush().await;
        }
    }
}

/// True when `path` is the root of a mounted filesystem.
///
/// * `path` - directory to test.
///
/// Compares the device number of `path` with that of `/`: a plain directory on
/// the live root shares it, a real mount point does not. Cheaper and more
/// honest than parsing `/proc/mounts`.
fn is_mountpoint(path: &str) -> bool {
    let (Ok(target), Ok(root)) = (std::fs::metadata(path), std::fs::metadata("/")) else {
        return false;
    };
    target.dev() != root.dev()
}

/// Copies the live log onto the target root, so it survives a reboot.
///
/// No-op unless [`crate::engine::INSTALL_ROOT`] is really a mount point:
/// otherwise the copy would land on the live tmpfs and pretend to be
/// persistent. Idempotent, so both the success path (before `PostInstallTask`
/// unmounts) and the failure path (the tee, once the pipeline aborted) can
/// call it.
///
/// # Post-conditions
/// On success [`TARGET_LOG`] exists and mirrors the live log at call time.
/// Every error is swallowed: a missing log copy must never fail an install.
pub async fn copy_to_target() {
    let Some(source) = path() else {
        return;
    };
    if !is_mountpoint(INSTALL_ROOT) {
        return;
    }
    if let Some(parent) = Path::new(TARGET_LOG).parent() {
        let _ = tokio::fs::create_dir_all(parent).await;
    }
    let _ = tokio::fs::copy(source, TARGET_LOG).await;
}
