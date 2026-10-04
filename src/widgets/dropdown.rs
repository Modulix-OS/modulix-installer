//! `GtkDropDown`'s popup is a virtualized `GtkListView` — it only measures
//! realized rows, so its natural width (and the button's) can visibly change
//! while scrolling through a long list instead of staying pinned to the
//! widest entry. `size_dropdown_to_widest` measures every string in the
//! model up front and pins the dropdown's minimum width to that, so it never
//! resizes regardless of scroll position or selection.

use adw::prelude::*;

/// Extra room for the popup's checkmark gutter and the dropdown arrow.
const CHROME_PADDING: i32 = 64;

pub fn size_dropdown_to_widest(dropdown: &impl IsA<gtk::Widget>, model: &gtk::StringList) {
    let widget = dropdown.upcast_ref::<gtk::Widget>();
    let mut max_width = 0;
    for i in 0..model.n_items() {
        if let Some(text) = model.string(i) {
            let layout = widget.create_pango_layout(Some(&text));
            max_width = max_width.max(layout.pixel_size().0);
        }
    }
    if max_width > 0 {
        widget.set_size_request(max_width + CHROME_PADDING, -1);
    }
}

/// Turns on type-ahead search for a dropdown backed by a [`gtk::StringList`].
///
/// `GtkDropDown` only shows a search entry when it knows how to turn an item
/// into text, which for a `StringList` means a property expression on
/// `GtkStringObject:string`. Without it the ~250 keyboard layouts, ~470
/// locales and ~400 timezones can only be reached by scrolling.
///
/// * `dropdown` - dropdown whose model is (or will be) a `gtk::StringList`.
///
/// # Post-conditions
/// The popup carries a search entry matching anywhere in the string, not just
/// at its start — "canada" has to find "French (Canada)".
pub fn enable_string_search(dropdown: &gtk::DropDown) {
    let expression = gtk::PropertyExpression::new(
        gtk::StringObject::static_type(),
        None::<gtk::Expression>,
        "string",
    );
    dropdown.set_expression(Some(expression));
    dropdown.set_enable_search(true);
    dropdown.set_search_match_mode(gtk::StringFilterMatchMode::Substring);
}
