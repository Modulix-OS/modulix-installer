//! Debug-only disk/encryption simulation, enabled by `--fake-disks=`/
//! `--fake-error=`/`--fake-instant` (see `cli`). `#[cfg(debug_assertions)]`
//! at the module declaration in `main.rs` keeps every byte of it out of a
//! release build — the live ISO links a release binary, so this never ships.
//!
//! This is **not** a fallback: `main.rs` only ever builds a `DebugSim` when
//! one of the three flags is given explicitly, and a failed
//! `Backends::new()`/`Backends::with_fake_disks()` stays fatal either way.
//! `eb6840b` removed the previous incarnation of this precisely because its
//! `main.rs` fell back to a fake backend *silently* on a D-Bus failure and
//! reported a simulated install as a success — see
//! `backend::Backends::new`'s doc comment.
//!
//! Only `disk` and `crypt` are faked. `network`/`locale`/`a11y` stay real
//! (`backend::Backends::with_fake_disks`): they are static system data or a
//! real D-Bus service already safe to exercise on a dev machine, not the
//! destructive operations this module exists to avoid.

pub mod cli;
mod crypt;
mod disks;
mod fault;
mod tasks;

pub use crypt::FakeCryptBackend;
pub use disks::FakeDiskBackend;
pub use fault::Fault;
pub use tasks::fake_pipeline;

use crate::backend::disk::scenario::DiskScenario;
use crate::engine::ProgressEvent;
use crate::mx;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

/// The single fault selected by `--fake-error=`, or none. A `struct` rather
/// than a bare `Option<Fault>` so `debugsim::disks`/`debugsim::crypt` can
/// share one `Arc` instead of each cloning the option.
pub struct FaultPlan {
    pub fault: Option<Fault>,
}

impl FaultPlan {
    pub fn is(&self, f: Fault) -> bool {
        self.fault == Some(f)
    }
}

/// Everything `--fake-disks=`/`--fake-error=`/`--fake-instant` configure,
/// installed once at startup into the global [`active`] slot. Immutable
/// after that: the three flags are read once, in `main`, before any window
/// exists.
pub struct DebugSim {
    pub disks: Arc<FakeDiskBackend>,
    pub crypt: Arc<FakeCryptBackend>,
    pub faults: Arc<FaultPlan>,
    /// `--fake-instant`: zero every `sim_sleep` wait in `tasks`, so a fake
    /// install completes in well under a second instead of ~3s.
    pub instant: bool,
    scenarios: Vec<DiskScenario>,
}

impl DebugSim {
    pub fn new(scenarios: Vec<DiskScenario>, fault: Option<Fault>, instant: bool) -> Self {
        let faults = Arc::new(FaultPlan { fault });
        let disks = Arc::new(FakeDiskBackend::new(&scenarios, faults.clone()));
        let crypt = Arc::new(FakeCryptBackend::new(faults.clone()));
        Self {
            disks,
            crypt,
            faults,
            instant,
            scenarios,
        }
    }

    /// Scenario names as given on the command line, for the simulation
    /// dialog `finish::summary` shows instead of the real destructive one.
    pub fn scenario_names(&self) -> Vec<&'static str> {
        self.scenarios.iter().map(|s| s.name()).collect()
    }

    /// `"none"` when no `--fake-error=` was given.
    pub fn fault_name(&self) -> &'static str {
        self.faults.fault.map(Fault::id).unwrap_or("none")
    }
}

static SIM: OnceLock<DebugSim> = OnceLock::new();

/// Installs the process-wide `DebugSim`. Called at most once, from `main`,
/// before any window is built — every later reader only ever sees this one
/// configuration. A second call is silently ignored.
pub fn install(sim: DebugSim) {
    let _ = SIM.set(sim);
}

/// `None` on a normal run connecting the real backends.
pub fn active() -> Option<&'static DebugSim> {
    SIM.get()
}

/// Rejects anything that is not a real device node — the same guard
/// `CryptsetupBackend::require_device_node` applies on the real path, so a
/// bug that leaks a udisks2 object path into `debugsim::disks`/
/// `debugsim::crypt` fails the same way it would against the real backends.
pub(crate) fn require_device_node(device: &str) -> mx::Result<()> {
    if device.starts_with("/dev/") {
        return Ok(());
    }
    Err(mx::Error::Backend(format!(
        "{device} is not a device node — a udisks2 object path must go through \
         DiskBackend::device_node before reaching the fake disk/crypt backend"
    )))
}

/// No-ops under `--fake-instant`; otherwise a real `tokio::time::sleep`, used
/// by `tasks`' fake doubles to give a run without that flag a
/// perceptible (~3s), not instantaneous, feel.
pub(crate) async fn sim_sleep(duration: Duration) {
    if !active().map(|sim| sim.instant).unwrap_or(false) {
        tokio::time::sleep(duration).await;
    }
}

/// Marks every log/status line with `[FAKE] `, so the on-screen log, the
/// install-log file and journald all carry the marker — there is no banner
/// anywhere else in the UI.
pub fn prefix_event(event: ProgressEvent) -> ProgressEvent {
    match event {
        ProgressEvent::Log(line) => ProgressEvent::Log(format!("[FAKE] {line}")),
        ProgressEvent::Started { task } => ProgressEvent::Started {
            task: format!("[FAKE] {task}"),
        },
        other => other,
    }
}
