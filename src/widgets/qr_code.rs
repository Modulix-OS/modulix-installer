//! QR code rendering, content-agnostic.
//!
//! Used by the install-progress page to turn a failed install's error report
//! into something a phone can read: the kiosk has no browser, no guaranteed
//! network and no way to get text out, so the alternative is copying Nix
//! traces and `/nix/store` paths by hand.
//!
//! A QR code holds at most [`QR_MAX_BYTES`]; whoever builds the payload is
//! responsible for staying under it (see [`crate::finish::qr_report`]).

use crate::i18n::tr;
use adw::prelude::*;
use qrcode::{Color, EcLevel, QrCode};

/// Largest payload a single QR code can carry: version 40, error-correction
/// level [`EcLevel::L`], byte mode.
pub const QR_MAX_BYTES: usize = 2953;

/// Quiet zone, in modules, mandated by the QR specification on each side.
/// Scanners rely on it; without it a code drawn edge-to-edge often fails.
const QUIET_MODULES: usize = 4;

/// Side of the QR drawing area, in pixels. A version-40 code is 177 modules
/// wide, so this leaves ~4 px per module — below that, phone cameras start to
/// miss the smallest payloads.
const VIEW_PX: i32 = 720;

/// A rendered QR code as a square matrix of modules.
pub struct QrMatrix {
    /// Side of the matrix in modules, quiet zone excluded.
    width: usize,
    /// Row-major, `width * width` entries; true means a dark module.
    dark: Vec<bool>,
}

/// Encodes `payload` into a QR matrix.
///
/// * `payload` - text to encode; UTF-8 is written in byte mode, which is what
///   phone scanners decode as UTF-8 in practice.
///
/// # Returns
/// `None` when the encoder refuses the payload — in practice only when it
/// exceeds [`QR_MAX_BYTES`], which callers are expected to prevent; this is a
/// safety net, not an expected path.
pub fn encode(payload: &str) -> Option<QrMatrix> {
    let code = QrCode::with_error_correction_level(payload.as_bytes(), EcLevel::L).ok()?;
    Some(QrMatrix {
        width: code.width(),
        dark: code
            .to_colors()
            .into_iter()
            .map(|c| c == Color::Dark)
            .collect(),
    })
}

/// Builds a `gtk::DrawingArea` that paints `matrix`.
///
/// * `matrix` - code to draw, consumed by the draw closure.
///
/// # Post-conditions
/// The code is always drawn as dark modules on a **white** background,
/// whatever the theme or the high-contrast setting: an inverted QR code is not
/// read by a large share of scanners. The module size is floored to whole
/// pixels and the matrix centred, because a fractional module renders blurry
/// and a blurry code does not scan.
fn drawing_area(matrix: QrMatrix) -> gtk::DrawingArea {
    let area = gtk::DrawingArea::builder()
        .content_width(VIEW_PX)
        .content_height(VIEW_PX)
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .accessible_role(gtk::AccessibleRole::Img)
        .build();
    area.update_property(&[gtk::accessible::Property::Label(&tr(
        "Error report QR code",
    ))]);

    area.set_draw_func(move |_area, cr, width, height| {
        let total = matrix.width + 2 * QUIET_MODULES;
        let module = (width.min(height) as usize / total.max(1)) as f64;
        let side = module * total as f64;
        let offset_x = (width as f64 - side) / 2.0;
        let offset_y = (height as f64 - side) / 2.0;

        cr.set_source_rgb(1.0, 1.0, 1.0);
        cr.rectangle(offset_x, offset_y, side, side);
        let _ = cr.fill();

        if module < 1.0 {
            return;
        }

        cr.set_source_rgb(0.0, 0.0, 0.0);
        for row in 0..matrix.width {
            for col in 0..matrix.width {
                if !matrix.dark[row * matrix.width + col] {
                    continue;
                }
                cr.rectangle(
                    offset_x + (col + QUIET_MODULES) as f64 * module,
                    offset_y + (row + QUIET_MODULES) as f64 * module,
                    module,
                    module,
                );
            }
        }
        let _ = cr.fill();
    });

    area
}

/// Opens a dialog showing `payload` as a QR code, with a copy-to-clipboard
/// fallback.
///
/// * `parent` - widget the dialog is presented on; any widget inside the
///   window works.
/// * `title` - dialog title.
/// * `caption` - one-paragraph explanation of what the code contains, shown
///   under it; already translated.
/// * `payload` - text to encode, expected to be at most [`QR_MAX_BYTES`].
///
/// # Post-conditions
/// The dialog is presented. When [`encode`] fails, the failure reason takes
/// the code's place rather than leaving an empty square, and the copy button
/// still works — the text is the point, the code is only the fast path.
pub fn present_dialog(parent: &impl IsA<gtk::Widget>, title: &str, caption: &str, payload: &str) {
    let content = gtk::Box::new(gtk::Orientation::Vertical, 18);
    content.set_margin_top(18);
    content.set_margin_bottom(18);
    content.set_margin_start(18);
    content.set_margin_end(18);

    match encode(payload) {
        Some(matrix) => content.append(&drawing_area(matrix)),
        None => {
            let failed = gtk::Label::builder()
                .label(tr("Could not build the QR code"))
                .wrap(true)
                .css_classes(["title-4"])
                .build();
            content.append(&failed);
        }
    }

    let caption_label = gtk::Label::builder()
        .label(caption)
        .wrap(true)
        .wrap_mode(gtk::pango::WrapMode::WordChar)
        .justify(gtk::Justification::Center)
        .max_width_chars(60)
        .halign(gtk::Align::Center)
        .css_classes(["dim-label"])
        .build();
    content.append(&caption_label);

    let copy_button = gtk::Button::builder()
        .label(tr("Copy text"))
        .css_classes(["pill"])
        .halign(gtk::Align::Center)
        .build();
    {
        let payload = payload.to_string();
        copy_button.connect_clicked(move |button| {
            button.clipboard().set_text(&payload);
        });
    }
    content.append(&copy_button);

    let scroller = gtk::ScrolledWindow::builder()
        .child(&content)
        .propagate_natural_width(true)
        .propagate_natural_height(true)
        .build();

    let view = adw::ToolbarView::new();
    view.add_top_bar(&adw::HeaderBar::new());
    view.set_content(Some(&scroller));

    let dialog = adw::Dialog::builder()
        .title(title)
        .child(&view)
        .content_width(VIEW_PX + 96)
        .content_height(VIEW_PX + 220)
        .build();
    dialog.present(Some(parent));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pins [`QR_MAX_BYTES`] to what the encoder really accepts: the whole
    /// report-building budget in `crate::finish::qr_report` depends on it.
    #[test]
    fn max_bytes_is_the_real_capacity() {
        assert!(encode(&"x".repeat(QR_MAX_BYTES)).is_some());
        assert!(encode(&"x".repeat(QR_MAX_BYTES + 1)).is_none());
    }

    #[test]
    fn matrix_is_square_and_version_40_at_capacity() {
        let matrix = encode(&"x".repeat(QR_MAX_BYTES)).unwrap();
        assert_eq!(matrix.width, 177);
        assert_eq!(matrix.dark.len(), matrix.width * matrix.width);
        // Top-left finder pattern: a dark 7x7 ring. Its corner module is dark
        // in every QR code, which is enough to catch a row/column mix-up.
        assert!(matrix.dark[0]);
    }
}
