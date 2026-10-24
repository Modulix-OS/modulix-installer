pub mod progress;
pub mod qr_report;
pub mod slides;
pub mod summary;

/// `adw::NavigationPage` tag of the pre-install review page
/// (`summary::SummaryPage`). Both finish pages are tagged for the same reason
/// the 9 steps are (`steps::StepId::tag`): `app.rs`'s single `visible-page`
/// handler maps the visible page back onto a step-rail row by tag, and the rail
/// now carries a row for each of them.
pub const SUMMARY_TAG: &str = "summary";

/// `adw::NavigationPage` tag of the install-progress page
/// (`progress::ProgressPage`). Reaching it is also what locks the whole rail —
/// see `app.rs`'s `installing` flag.
pub const PROGRESS_TAG: &str = "install";
