//! Error report carried by the failure page's QR code.
//!
//! A QR code holds at most [`QR_MAX_BYTES`] bytes and a `nixos-install` log is
//! hundreds of kilobytes, so the report is the error message, the paths of the
//! persisted log copies, and the **tail** of the log — the part that contains
//! the failure — cut down to fit, with an explicit marker saying how much was
//! dropped.
//!
//! The report is deliberately **untranslated**, for the same reason
//! [`crate::engine::Task::label`] and [`crate::engine::install_log`] are: it is
//! read by whoever diagnoses the install, not shown on screen.

use crate::widgets::qr_code::QR_MAX_BYTES;

/// First line of every report, so a scanned payload identifies itself.
const HEADER: &str = "ModulixOS install failed";

/// Separates the header block from the log tail.
const TAIL_SEPARATOR: &str = "--- log tail ---";

/// Builds the QR payload for a failed install.
///
/// * `error` - user-facing failure reason, as rendered on the page.
/// * `paths` - already-formatted lines naming each persisted log copy; may be
///   empty when nothing could be written.
/// * `log` - full install log as read back from disk; may be empty.
///
/// # Post-conditions
/// The returned string is at most [`QR_MAX_BYTES`] bytes long, so
/// [`crate::widgets::qr_code::encode`] always accepts it. The log tail is cut
/// on a UTF-8 character boundary and, whenever the log has more than one line,
/// on a line boundary. The `(truncated, N bytes omitted)` marker appears if and
/// only if part of the log was dropped.
pub fn build_report(error: &str, paths: &[String], log: &str) -> String {
    let mut head = String::with_capacity(QR_MAX_BYTES);
    head.push_str(HEADER);
    head.push('\n');
    let error = error.trim();
    if !error.is_empty() {
        head.push_str(error);
        head.push('\n');
    }
    for path in paths {
        head.push_str(path.trim());
        head.push('\n');
    }

    if log.trim().is_empty() {
        return fit(head);
    }

    // Reserve the marker up front at its worst-case length: the byte count it
    // reports can never exceed the whole log's length.
    let marker_reserve = marker(log.len()).len();
    let fixed = head.len() + TAIL_SEPARATOR.len() + 1 + marker_reserve;
    let Some(available) = QR_MAX_BYTES.checked_sub(fixed) else {
        return fit(head);
    };

    let tail = tail_of(log, available);
    if tail.is_empty() {
        return fit(head);
    }

    let mut report = head;
    report.push_str(TAIL_SEPARATOR);
    report.push('\n');
    report.push_str(tail);
    if !report.ends_with('\n') {
        report.push('\n');
    }
    let omitted = log.len() - tail.len();
    if omitted > 0 {
        report.push_str(&marker(omitted));
    }
    fit(report)
}

/// Marker appended when the log did not fit whole.
///
/// * `omitted` - number of bytes dropped from the head of the log.
fn marker(omitted: usize) -> String {
    format!("(truncated, {omitted} bytes omitted)\n")
}

/// Last `budget` bytes of `log`, starting on a character boundary and, when
/// possible, at the start of a line.
///
/// * `log` - full log text.
/// * `budget` - maximum number of bytes to return.
///
/// # Post-conditions
/// The result is a non-empty suffix of `log` (for a non-empty `log`) of at
/// most `budget` bytes. Mid-line cuts only happen when aligning on a line
/// would leave nothing at all — a log whose tail holds no newline, or a single
/// line longer than the budget.
fn tail_of(log: &str, budget: usize) -> &str {
    let mut start = log.len().saturating_sub(budget);
    while start < log.len() && !log.is_char_boundary(start) {
        start += 1;
    }
    if start > 0
        && let Some(newline) = log[start..].find('\n')
        && start + newline + 1 < log.len()
    {
        start += newline + 1;
    }
    &log[start..]
}

/// Truncates `report` to [`QR_MAX_BYTES`] on a character boundary.
///
/// * `report` - payload to bound; returned unchanged when it already fits.
///
/// Last-resort guard: it only fires when the fixed part alone (a huge Nix
/// error message) exceeds the capacity of a QR code.
fn fit(mut report: String) -> String {
    if report.len() <= QR_MAX_BYTES {
        return report;
    }
    let mut end = QR_MAX_BYTES;
    while end > 0 && !report.is_char_boundary(end) {
        end -= 1;
    }
    report.truncate(end);
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths() -> Vec<String> {
        vec![
            "Full log: /var/log/modulixos-install.log".to_string(),
            "Copy kept on the target disk: /mnt/var/log/modulixos-install.log".to_string(),
        ]
    }

    #[test]
    fn short_log_is_kept_whole_without_marker() {
        let log = "line one\nline two\nerror: boom\n";
        let report = build_report("could not format /dev/sda2", &paths(), log);
        assert!(report.starts_with(HEADER));
        assert!(report.contains("could not format /dev/sda2"));
        assert!(report.contains("line one"));
        assert!(report.contains("error: boom"));
        assert!(!report.contains("truncated"));
        assert!(report.len() <= QR_MAX_BYTES);
    }

    #[test]
    fn long_log_is_cut_to_the_tail_and_marked() {
        let log: String = (0..20_000).map(|i| format!("line {i}\n")).collect();
        let report = build_report("nixos-install failed", &paths(), &log);
        assert!(report.len() <= QR_MAX_BYTES);
        assert!(report.contains("line 19999"));
        assert!(!report.contains("line 0\n"));
        assert!(report.contains("bytes omitted)"));
        // The cut is line aligned: the first log line in the report is whole.
        let tail = report.split(TAIL_SEPARATOR).nth(1).unwrap();
        let first = tail.trim_start_matches('\n').lines().next().unwrap();
        assert!(first.starts_with("line "), "cut mid-line: {first:?}");
    }

    #[test]
    fn empty_log_yields_header_and_paths_only() {
        let report = build_report("disk vanished", &paths(), "   \n");
        assert_eq!(
            report,
            format!(
                "{HEADER}\ndisk vanished\nFull log: /var/log/modulixos-install.log\n\
                 Copy kept on the target disk: /mnt/var/log/modulixos-install.log\n"
            )
        );
    }

    #[test]
    fn oversized_error_is_bounded() {
        let error = "e".repeat(QR_MAX_BYTES * 2);
        let report = build_report(&error, &paths(), "log line\n");
        assert_eq!(report.len(), QR_MAX_BYTES);
    }

    #[test]
    fn multibyte_log_is_cut_on_a_character_boundary() {
        // No newline at all: forces the mid-line fallback, which still has to
        // land on a character boundary.
        let log = "é".repeat(10_000);
        let report = build_report("boom", &paths(), &log);
        assert!(report.len() <= QR_MAX_BYTES);
        assert!(report.contains('é'));
        // A panic-free `contains` already proves the string is valid UTF-8;
        // round-tripping the bytes proves the cut did not split a code point.
        assert!(std::str::from_utf8(report.as_bytes()).is_ok());
    }

    #[test]
    fn single_huge_line_still_fits() {
        let log = format!("{}\n", "x".repeat(50_000));
        let report = build_report("boom", &paths(), &log);
        assert!(report.len() <= QR_MAX_BYTES);
        assert!(report.contains("bytes omitted)"));
    }

    #[test]
    fn no_paths_is_accepted() {
        let report = build_report("boom", &[], "a\nb\n");
        assert!(report.starts_with(&format!("{HEADER}\nboom\n")));
        assert!(report.contains("a\nb\n"));
    }
}
