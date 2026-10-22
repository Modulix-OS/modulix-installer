//! Geometry shared by the two card-picker steps — 8 (desktop environment) and
//! 9 (applications). Both lay a `gtk::FlowBox` of fixed-aspect cards inside an
//! `adw::Clamp` inside an `adw::BreakpointBin`, with one breakpoint per column
//! count.
//!
//! Each step keeps its own card min/max width: step 9's cards are smaller
//! because there are seven of them against step 8's one to four, and three
//! step-8-sized cards do not fit the logical width the kiosk guarantees (see
//! `nix/kiosk-module.nix`). Only the spacing, the page margin and the
//! breakpoint plumbing are common, so only those live here.

use adw::prelude::*;

/// Gap between two cards, horizontally and vertically.
pub(crate) const CARD_SPACING: i32 = 12;
/// Margin between the page content and the window edge, on all four sides.
pub(crate) const PAGE_MARGIN: i32 = 24;

/// Width of an N-card row at the card's maximum width — the `adw::Clamp`
/// ceiling for that column count.
///
/// # Parameters
/// * `columns` - number of cards per line; at least 1.
/// * `card_max` - the step's maximum card width in pixels.
///
/// # Returns
/// `columns` cards plus the `columns - 1` gaps between them, in pixels.
pub(crate) const fn grid_width(columns: i32, card_max: i32) -> i32 {
    columns * card_max + (columns - 1) * CARD_SPACING
}

/// Narrowest `adw::BreakpointBin` width at which `columns` cards still fit at
/// their minimum width, page margins included. A breakpoint threshold must be
/// this or above, never below.
///
/// # Parameters
/// * `columns` - number of cards per line; at least 1.
/// * `card_min` - the step's minimum card width in pixels.
///
/// # Returns
/// The required bin width in pixels.
pub(crate) const fn column_min_width(columns: i32, card_min: i32) -> i32 {
    columns * card_min + (columns - 1) * CARD_SPACING + 2 * PAGE_MARGIN
}

/// Adds one rung to a card grid's column ladder: from `min_width` upwards,
/// `grid` lays out exactly `columns` children per line and `clamp` widens to
/// that row's full width.
///
/// # Parameters
/// * `bin` - the `adw::BreakpointBin` whose width drives the condition.
/// * `grid` - the card `gtk::FlowBox`, whose children-per-line is set.
/// * `clamp` - the `adw::Clamp` around the page content, whose maximum size is
///   set to [`grid_width`] for this column count.
/// * `min_width` - threshold in pixels, at or above
///   `column_min_width(columns, card_min)`.
/// * `columns` - cards per line above the threshold; at least 1.
/// * `card_max` - the step's maximum card width in pixels.
///
/// # Preconditions
/// Rungs must be added in ascending `min_width` order: `adw::BreakpointBin`
/// applies the last matching breakpoint, not the narrowest.
pub(crate) fn add_column_breakpoint(
    bin: &adw::BreakpointBin,
    grid: &gtk::FlowBox,
    clamp: &adw::Clamp,
    min_width: i32,
    columns: i32,
    card_max: i32,
) {
    let breakpoint = adw::Breakpoint::new(adw::BreakpointCondition::new_length(
        adw::BreakpointConditionLengthType::MinWidth,
        min_width as f64,
        adw::LengthUnit::Px,
    ));
    let per_line = (columns as u32).to_value();
    breakpoint.add_setter(grid, "min-children-per-line", Some(&per_line));
    breakpoint.add_setter(grid, "max-children-per-line", Some(&per_line));
    breakpoint.add_setter(
        clamp,
        "maximum-size",
        Some(&grid_width(columns, card_max).to_value()),
    );
    bin.add_breakpoint(breakpoint);
}
