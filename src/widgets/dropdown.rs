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
