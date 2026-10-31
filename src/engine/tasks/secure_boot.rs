use crate::engine::tasks::capture;
use crate::engine::{INSTALL_ROOT, ProgressEvent, ProgressSink, Task, TaskCtx};
use crate::mx;
use async_trait::async_trait;
use modulix_core_utils::secure_boot::{self, FirmwareState};

/// Absolute path of `sbctl` inside the installed system. `nixos-enter -- …`
/// execs the command directly, with no PATH of its own, so the system profile
/// has to be spelled out — same path `FlatpakRemoteTask` uses for `flatpak`.
/// mxpkgs puts `sbctl` in `environment.systemPackages` whenever
/// `mx.bootloader.secureBoot.enable` is set, which is exactly when this stage
/// does anything.
const SBCTL_BIN: &str = "/nix/var/nix/profiles/system/sw/bin/sbctl";

/// Checks that the Secure Boot key enrollment `nixos-install` was supposed to
/// perform actually happened, and warns when it did not.
///
/// This stage exists because the failure it looks for is **silent**. When
/// `mx.bootloader.secureBoot.enable` is set, nixpkgs' `limine-install.py`
/// runs `sbctl create-keys`, `sbctl enroll-keys` and `sbctl sign` — each
/// through `subprocess.run` **without** `check=True`. A non-zero exit is
/// ignored, no exception is raised, and the install reports success on a
/// system whose loader is unsigned and whose firmware never received a key.
/// Nothing else in the pipeline would ever notice.
///
/// Guarded on `PipelineState::secure_boot_requested`, recorded by
/// `InitConfigTask` *before* the build: enrolling a Platform Key is what takes
/// the firmware out of Setup Mode, so by the time this runs the live firmware
/// state can no longer distinguish "enrolled successfully" from "was never
/// eligible". The guard is therefore a recorded answer, never a fresh probe of
/// the target — the same rule that applies to every stage here.
///
/// Best-effort from end to end: the system is already on disk, and an
/// unverified enrollment costs the user a firmware menu visit, not an install.
/// Every outcome returns `Ok(())`; the bad ones go out as
/// [`ProgressEvent::Warning`], which `finish::progress` shows next to
/// "Installation complete".
pub struct SecureBootTask;

/// What `sbctl status --json` reported, reduced to the two fields that
/// decide the outcome.
///
/// Parsed by hand rather than through `serde`: two booleans out of a flat
/// object is not worth a derive, and this stage must not fail on a field
/// `sbctl` adds in a later release.
struct SbctlStatus {
    /// `true` while no Platform Key is enrolled, i.e. the enrollment did not
    /// happen.
    setup_mode: bool,
    /// Whether the firmware is currently enforcing Secure Boot. Expected to
    /// be `false` right after the install — the user still has to switch it
    /// on in the firmware setup menu.
    secure_boot: bool,
}

/// Reads one top-level boolean out of `sbctl status --json`.
///
/// * `json` - the command's stdout.
/// * `key` - the field name, without quotes.
///
/// # Returns
/// The field's value, or `None` when the field is absent or is not a bare
/// `true`/`false`.
fn json_bool(json: &str, key: &str) -> Option<bool> {
    let needle = format!("\"{key}\"");
    let rest = json.split_once(&needle)?.1;
    let rest = rest.trim_start().strip_prefix(':')?.trim_start();
    if rest.starts_with("true") {
        Some(true)
    } else if rest.starts_with("false") {
        Some(false)
    } else {
        None
    }
}

/// Parses the two fields this stage reads out of `sbctl status --json`.
///
/// * `json` - the command's stdout.
///
/// # Returns
/// The parsed status, or `None` when either field is missing — which is
/// treated as "could not tell", not as a failure of the enrollment.
fn parse_status(json: &str) -> Option<SbctlStatus> {
    Some(SbctlStatus {
        setup_mode: json_bool(json, "setup_mode")?,
        secure_boot: json_bool(json, "secure_boot")?,
    })
}

/// Fallback verdict when `sbctl` could not be reached, read off the **live**
/// firmware rather than the target.
///
/// Enrolling a Platform Key takes the platform out of Setup Mode, and it is
/// the same firmware the installer itself booted from, so the `SetupMode`
/// variable answers the question on its own. This is not a host-side probe of
/// the installed system — nothing under `/mnt` is read.
///
/// # Returns
/// `Some(true)` when the firmware left Setup Mode, i.e. a key was enrolled;
/// `Some(false)` when it did not; `None` when the state could not be read.
fn enrolled_per_firmware() -> Option<bool> {
    match secure_boot::firmware_state() {
        Ok(FirmwareState::SetupMode) => Some(false),
        Ok(FirmwareState::UserMode) => Some(true),
        Ok(FirmwareState::NotUefi) | Err(_) => None,
    }
}

#[async_trait]
impl Task for SecureBootTask {
    fn label(&self) -> String {
        "Checking secure boot".to_string()
    }

    fn weight(&self) -> u32 {
        1
    }

    /// # Pre-conditions
    /// `NixosInstallTask` has completed and the target is still mounted at
    /// `/mnt`, which is why this stage precedes `PostInstallTask`.
    ///
    /// # Post-conditions
    /// Either the install log records that the keys were enrolled, or a
    /// [`ProgressEvent::Warning`] says they were not and what that means for
    /// the user. Nothing is written anywhere and no firmware variable is
    /// touched: this stage only reads.
    ///
    /// # Returns
    /// Always `Ok(())` — see the type-level doc.
    async fn run(&self, ctx: &TaskCtx, tx: &ProgressSink) -> mx::Result<()> {
        if !ctx.state.lock().await.secure_boot_requested {
            let _ = tx
                .send(ProgressEvent::Log(
                    "secure boot: skipped (it was not enabled for this install)".into(),
                ))
                .await;
            return Ok(());
        }

        // Arguments as an array, never a shell: this runs as root.
        let outcome = capture(
            "nixos-enter",
            &["--root", INSTALL_ROOT, "--", SBCTL_BIN, "status", "--json"],
        )
        .await;

        let status = match &outcome {
            Ok(json) => parse_status(json),
            Err(_) => None,
        };

        // The diagnostic detail goes out as a log line and the warning itself
        // stays a fixed sentence: `finish::progress` runs the warning through
        // `tr`, and an interpolated string matches no msgid.
        let enrolled = match status {
            Some(status) => {
                let _ = tx
                    .send(ProgressEvent::Log(format!(
                        "secure boot: sbctl reports setup_mode={}, secure_boot={}",
                        status.setup_mode, status.secure_boot
                    )))
                    .await;
                Some(!status.setup_mode)
            }
            None => {
                let reason = match &outcome {
                    Ok(_) => "its output could not be parsed".to_string(),
                    Err(e) => e.clone(),
                };
                let _ = tx
                    .send(ProgressEvent::Log(format!(
                        "secure boot: sbctl could not be read ({reason}), \
                         falling back to the firmware's own setup-mode variable"
                    )))
                    .await;
                enrolled_per_firmware()
            }
        };

        let event = match enrolled {
            Some(true) => ProgressEvent::Log(
                "secure boot: keys enrolled and the bootloader signed; turn secure boot \
                 on in the firmware setup menu to start enforcing it"
                    .into(),
            ),
            Some(false) => ProgressEvent::Warning(WARNING_NOT_ENROLLED.to_string()),
            None => ProgressEvent::Warning(WARNING_UNVERIFIED.to_string()),
        };

        let _ = tx.send(event).await;
        Ok(())
    }
}

/// Shown when the enrollment demonstrably did not happen.
///
/// Untranslated here on purpose, like every other pipeline string: tasks run
/// on tokio worker threads and the `LANGUAGE` slot is read from the GTK main
/// thread only. `finish::progress` calls `tr()` on arrival, which is also why
/// this is a fixed literal and not a `format!` — an interpolated sentence
/// matches no msgid.
pub(crate) const WARNING_NOT_ENROLLED: &str = "Secure Boot could not be set up: no keys were enrolled \
    and the bootloader is unsigned. Leave Secure Boot disabled in the firmware setup menu, \
    or the installed system will not start.";

/// Shown when the outcome could not be established either way. The reason is
/// in the install log, as a separate line; see [`WARNING_NOT_ENROLLED`] for
/// why this one carries none.
const WARNING_UNVERIFIED: &str = "Secure Boot was set up but the result could not be \
    verified. Run `sbctl status` on the installed system before enabling Secure Boot in \
    the firmware setup menu.";

#[cfg(test)]
mod tests {
    use super::*;

    /// A real `sbctl status --json` payload, as sbctl 0.18 prints it.
    const REAL_OUTPUT: &str = r#"{
  "installed": true,
  "guid": "19962d9f-8b20-4bb5-9dc1-077e9362d876",
  "setup_mode": false,
  "secure_boot": false,
  "vendors": [
    "microsoft",
    "builtin-db",
    "builtin-KEK"
  ],
  "firmware_quirks": []
}"#;

    #[test]
    fn a_real_payload_parses() {
        let status = parse_status(REAL_OUTPUT).expect("the real payload must parse");
        assert!(!status.setup_mode);
        assert!(!status.secure_boot);
    }

    #[test]
    fn both_boolean_values_are_read() {
        let json = r#"{"setup_mode": true, "secure_boot": false}"#;
        let status = parse_status(json).expect("must parse");
        assert!(status.setup_mode);
        assert!(!status.secure_boot);
    }

    /// Whitespace around the colon is valid JSON and sbctl is free to change
    /// its formatting between releases.
    #[test]
    fn spacing_around_the_colon_is_tolerated() {
        assert_eq!(
            json_bool(r#"{"setup_mode"   :   true}"#, "setup_mode"),
            Some(true)
        );
        assert_eq!(
            json_bool(r#"{"setup_mode":false}"#, "setup_mode"),
            Some(false)
        );
    }

    /// A missing field must read as "could not tell", never as a verdict: the
    /// caller then falls back to the firmware variable instead of warning on
    /// a payload it simply failed to understand.
    #[test]
    fn a_missing_field_yields_no_status() {
        assert!(parse_status(r#"{"setup_mode": true}"#).is_none());
        assert!(parse_status(r#"{"secure_boot": true}"#).is_none());
        assert!(parse_status("not json at all").is_none());
    }

    /// Same rule for a field whose value is not a bare boolean — `null` is
    /// what sbctl would print for something it could not determine.
    #[test]
    fn a_non_boolean_value_yields_no_status() {
        assert_eq!(json_bool(r#"{"setup_mode": null}"#, "setup_mode"), None);
        assert_eq!(json_bool(r#"{"setup_mode": "yes"}"#, "setup_mode"), None);
    }

    /// The exact text of both warnings and of the task label, pinned because
    /// `tests/po_coverage.rs` cannot see them: it only reads *literal*
    /// arguments of `tr`, and these three reach it through a constant. A
    /// reflow that changes a runtime byte would otherwise silently drop the
    /// translation back to English.
    #[test]
    fn the_translated_strings_match_the_catalogs() {
        assert_eq!(SecureBootTask.label(), "Checking secure boot");
        assert_eq!(
            WARNING_NOT_ENROLLED,
            "Secure Boot could not be set up: no keys were enrolled and the bootloader is \
             unsigned. Leave Secure Boot disabled in the firmware setup menu, or the \
             installed system will not start."
        );
        assert_eq!(
            WARNING_UNVERIFIED,
            "Secure Boot was set up but the result could not be verified. Run `sbctl \
             status` on the installed system before enabling Secure Boot in the firmware \
             setup menu."
        );
    }

    /// The two warnings are the only user-facing strings this stage produces,
    /// and `finish::progress` runs them through `tr`. An interpolated one
    /// would match no msgid, so they must stay whole, fixed sentences.
    #[test]
    fn the_warnings_are_translatable_sentences() {
        for warning in [WARNING_NOT_ENROLLED, WARNING_UNVERIFIED] {
            assert!(!warning.is_empty());
            assert!(!warning.contains('{'), "{warning}");
            assert!(warning.ends_with('.'), "{warning}");
        }
    }
}
