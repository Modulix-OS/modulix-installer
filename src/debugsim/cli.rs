//! Manual `std::env::args()` parsing for the debug-only `--fake-disks=`,
//! `--fake-error=` and `--fake-instant` flags. No `clap`: these three flags
//! are the entire surface, and pulling in a CLI framework for them would be
//! the only new dependency this feature needs.

use super::fault::{self, Fault};
use crate::backend::disk::scenario::DiskScenario;

pub const USAGE: &str = "usage: modulixos-installer [--fake-disks=<scenario>[,<scenario>...]] [--fake-error=<id>|list|random] [--fake-instant]\n\
run with no arguments to connect the real backends.";

#[derive(Debug, Clone)]
pub struct FakeConfig {
    pub disks: Vec<DiskScenario>,
    pub fault: Option<Fault>,
    pub instant: bool,
}

#[derive(Debug, Clone)]
pub enum CliOutcome {
    /// No `--fake*` flag was given: connect the real backends.
    Real,
    Fake(FakeConfig),
    /// `--fake-error=list` — print and exit 0 before touching GTK/tokio.
    PrintAndExit(String),
    /// A malformed flag, or an unrecognized `--`-prefixed argument.
    Error(String),
}

fn parse_disk_list(raw: &str) -> Result<Vec<DiskScenario>, String> {
    raw.split(',')
        .map(|name| {
            DiskScenario::parse(name).ok_or_else(|| {
                format!(
                    "unknown --fake-disks scenario: {name}\nvalid names: {}\n{USAGE}",
                    DiskScenario::ALL
                        .iter()
                        .map(|s| s.name())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })
        })
        .collect()
}

pub fn parse(args: &[String]) -> CliOutcome {
    let mut disks: Option<Vec<DiskScenario>> = None;
    let mut fault_raw: Option<String> = None;
    let mut instant = false;
    let mut any_fake = false;

    for arg in args {
        if let Some(v) = arg.strip_prefix("--fake-disks=") {
            any_fake = true;
            match parse_disk_list(v) {
                Ok(list) => disks = Some(list),
                Err(e) => return CliOutcome::Error(e),
            }
        } else if let Some(v) = arg.strip_prefix("--fake-error=") {
            any_fake = true;
            if v == "list" {
                return CliOutcome::PrintAndExit(fault::catalog_text());
            }
            fault_raw = Some(v.to_string());
        } else if arg == "--fake-instant" {
            any_fake = true;
            instant = true;
        } else if arg.starts_with("--") {
            return CliOutcome::Error(format!("unknown flag: {arg}\n{USAGE}"));
        }
    }

    if !any_fake {
        return CliOutcome::Real;
    }

    let fault = match fault_raw {
        None => None,
        Some(raw) if raw == "random" => Some(Fault::random()),
        Some(raw) => match Fault::parse(&raw) {
            Some(f) => Some(f),
            None => {
                return CliOutcome::Error(format!(
                    "unknown --fake-error id: {raw}\nrun --fake-error=list to see the catalog\n{USAGE}"
                ));
            }
        },
    };

    let disks = disks.unwrap_or_else(|| vec![DiskScenario::MultiDisk]);

    CliOutcome::Fake(FakeConfig {
        disks,
        fault,
        instant,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &[&str]) -> Vec<String> {
        s.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn no_flags_is_real() {
        assert!(matches!(parse(&args(&[])), CliOutcome::Real));
    }

    #[test]
    fn fake_disks_parses_scenarios() {
        let outcome = parse(&args(&["--fake-disks=empty,bitlocker"]));
        match outcome {
            CliOutcome::Fake(cfg) => {
                assert_eq!(
                    cfg.disks,
                    vec![DiskScenario::Empty, DiskScenario::BitLocker]
                );
                assert_eq!(cfg.fault, None);
                assert!(!cfg.instant);
            }
            other => panic!("expected Fake, got {other:?}"),
        }
    }

    #[test]
    fn aliases_resolve_inside_fake_disks() {
        let outcome = parse(&args(&["--fake-disks=fragmented,half-partitioned"]));
        match outcome {
            CliOutcome::Fake(cfg) => assert_eq!(
                cfg.disks,
                vec![DiskScenario::Fragmented, DiskScenario::FreeSpaceTail]
            ),
            other => panic!("expected Fake, got {other:?}"),
        }
    }

    #[test]
    fn unknown_scenario_is_an_error() {
        assert!(matches!(
            parse(&args(&["--fake-disks=not-a-scenario"])),
            CliOutcome::Error(_)
        ));
    }

    #[test]
    fn missing_fake_disks_defaults_to_multi_disk_when_another_fake_flag_is_set() {
        let outcome = parse(&args(&["--fake-instant"]));
        match outcome {
            CliOutcome::Fake(cfg) => {
                assert_eq!(cfg.disks, vec![DiskScenario::MultiDisk]);
                assert!(cfg.instant);
            }
            other => panic!("expected Fake, got {other:?}"),
        }
    }

    #[test]
    fn fake_error_list_prints_and_exits() {
        assert!(matches!(
            parse(&args(&["--fake-error=list"])),
            CliOutcome::PrintAndExit(_)
        ));
    }

    #[test]
    fn fake_error_random_resolves_to_some_fault() {
        match parse(&args(&["--fake-error=random"])) {
            CliOutcome::Fake(cfg) => assert!(cfg.fault.is_some()),
            other => panic!("expected Fake, got {other:?}"),
        }
    }

    #[test]
    fn malformed_fake_error_is_an_error() {
        assert!(matches!(
            parse(&args(&["--fake-error=not:a-fault"])),
            CliOutcome::Error(_)
        ));
    }

    #[test]
    fn unknown_dashdash_flag_is_an_error() {
        assert!(matches!(parse(&args(&["--bogus"])), CliOutcome::Error(_)));
    }

    #[test]
    fn non_dashdash_arguments_are_ignored() {
        assert!(matches!(parse(&args(&["positional"])), CliOutcome::Real));
    }
}
