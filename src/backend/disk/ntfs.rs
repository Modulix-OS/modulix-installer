//! NTFS shrink preflight for `AlongsideWindows` mode — mandatory checks
//! before ever touching an NTFS volume: BitLocker, hibernation, a
//! dirty volume, or `ntfsresize` refusing outright. BitLocker is detected
//! upstream from `Block.IdType == "BitLocker"` without even invoking the
//! tool (see `udisks2.rs::probe_ntfs`); this module only parses
//! `ntfsresize`'s own output.

#[derive(Debug, Clone, PartialEq)]
pub struct NtfsProbe {
    /// Smallest size `ntfsresize` reports the volume could be shrunk to.
    pub min_size_bytes: u64,
    /// The volume's actual current size.
    pub current_size_bytes: u64,
    pub blockers: Vec<NtfsBlocker>,
    /// Bytes actually in use inside the volume, from `ntfsresize`'s
    /// `Space in use` line — `None` if that line wasn't printed (e.g. the
    /// probe failed before reaching cluster accounting).
    pub used_bytes: Option<u64>,
}

impl NtfsProbe {
    pub fn is_shrinkable(&self) -> bool {
        self.blockers.is_empty()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NtfsBlocker {
    BitLocker,
    Hibernated,
    DirtyVolume,
    ResizeUnsupported,
}

/// Parses `ntfsresize --info --no-action --force <dev>` output (args as an
/// array — never through a shell). `exit_ok` is the process's
/// exit status; a nonzero exit with no other recognized blocker still
/// surfaces as [`NtfsBlocker::ResizeUnsupported`] so a probe failure never
/// silently reads as "shrinkable".
pub fn parse_ntfsresize_info(stdout: &str, stderr: &str, exit_ok: bool) -> NtfsProbe {
    let combined = format!("{stdout}\n{stderr}");
    let lower = combined.to_lowercase();

    let mut blockers = Vec::new();
    if lower.contains("hibernated") || lower.contains("hibernation") {
        blockers.push(NtfsBlocker::Hibernated);
    }
    if lower.contains("dirty") || lower.contains("scheduled for a check") {
        blockers.push(NtfsBlocker::DirtyVolume);
    }

    let min_size_bytes = find_bytes_after(&combined, "You might resize at")
        .or_else(|| find_bytes_after(&combined, "resize at"))
        .unwrap_or(0);
    let current_size_bytes = find_bytes_after(&combined, "Current volume size:").unwrap_or(0);
    // "Space in use       : 210000 MB (52.5%)" — unlike the other two
    // markers this one is reported in MB, not bytes.
    let used_bytes = find_bytes_after(&combined, "Space in use").map(|mb| mb * 1024 * 1024);

    if !exit_ok && blockers.is_empty() {
        blockers.push(NtfsBlocker::ResizeUnsupported);
    }
    // `ntfsresize --info` on a volume already at its floor exits 0 without
    // ever printing "You might resize at" — an unparsed `min_size_bytes`
    // must never read as "shrinkable to zero".
    if exit_ok && blockers.is_empty() && min_size_bytes == 0 {
        blockers.push(NtfsBlocker::ResizeUnsupported);
    }

    NtfsProbe {
        min_size_bytes,
        current_size_bytes,
        blockers,
        used_bytes,
    }
}

/// Finds the first integer token after `marker` — `ntfsresize` reports sizes
/// as e.g. `"You might resize at 225485783040 bytes"`.
fn find_bytes_after(text: &str, marker: &str) -> Option<u64> {
    let idx = text.find(marker)?;
    let rest = &text[idx + marker.len()..];
    rest.split_whitespace().find_map(|tok| {
        let digits: String = tok.chars().filter(|c| c.is_ascii_digit()).collect();
        if digits.is_empty() {
            None
        } else {
            digits.parse().ok()
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_output_is_shrinkable() {
        let stdout = "Device name        : /dev/sda3\n\
                       NTFS volume version: 3.1\n\
                       Cluster size       : 4096 bytes\n\
                       Current volume size: 429496729600 bytes (430 GB)\n\
                       Current device size: 429496729600 bytes (430 GB)\n\
                       You might resize at 225485783040 bytes or 225486 MB (freeing 204010946560 bytes)\n\
                       Please make a test run using both the -n and -s options before real resizing!\n";
        let probe = parse_ntfsresize_info(stdout, "", true);
        assert!(probe.is_shrinkable());
        assert_eq!(probe.min_size_bytes, 225485783040);
        assert_eq!(probe.current_size_bytes, 429496729600);
    }

    #[test]
    fn space_in_use_is_parsed_from_mb_to_bytes() {
        let stdout = "Current volume size: 429496729600 bytes (430 GB)\n\
                       Space in use       : 210000 MB (48.9%)\n\
                       You might resize at 225485783040 bytes or 225486 MB\n";
        let probe = parse_ntfsresize_info(stdout, "", true);
        assert_eq!(probe.used_bytes, Some(210000 * 1024 * 1024));
    }

    #[test]
    fn missing_space_in_use_line_is_none() {
        let probe = parse_ntfsresize_info("", "", true);
        assert_eq!(probe.used_bytes, None);
    }

    #[test]
    fn dirty_volume_is_blocked() {
        let stderr = "Volume is scheduled for a check.\n\
                       Please boot into Windows TWICE, or\nuse the Disks utility.\n";
        let probe = parse_ntfsresize_info("", stderr, false);
        assert!(probe.blockers.contains(&NtfsBlocker::DirtyVolume));
        assert!(!probe.is_shrinkable());
    }

    #[test]
    fn hibernated_volume_is_blocked() {
        let stderr = "Windows is hibernated, refused to mount.\n\
                       Failed to mount '/dev/sda3': Operation not permitted\n\
                       The NTFS partition is in an unsafe state.\n";
        let probe = parse_ntfsresize_info("", stderr, false);
        assert!(probe.blockers.contains(&NtfsBlocker::Hibernated));
        assert!(!probe.is_shrinkable());
    }

    #[test]
    fn resize_unsupported_falls_back_when_no_marker_matched() {
        let stderr = "ntfsresize v2022.10.3\n\
                       Failed to determine the type of the source volume\n";
        let probe = parse_ntfsresize_info("", stderr, false);
        assert_eq!(probe.blockers, vec![NtfsBlocker::ResizeUnsupported]);
    }

    #[test]
    fn successful_exit_with_no_markers_is_resize_unsupported() {
        // e.g. "Nothing to do" output — the volume is already at its floor,
        // so `min_size_bytes` stays unparsed (0), which must never read as
        // "shrinkable without limit".
        let probe = parse_ntfsresize_info("", "", true);
        assert_eq!(probe.blockers, vec![NtfsBlocker::ResizeUnsupported]);
        assert_eq!(probe.min_size_bytes, 0);
        assert!(!probe.is_shrinkable());
    }
}
