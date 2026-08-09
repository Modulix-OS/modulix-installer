//! Proportional bar showing a disk's partition layout (partitioning step).

use gtk::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;

const PALETTE: &[(f64, f64, f64)] = &[
    (0.31, 0.60, 0.96),
    (0.98, 0.68, 0.14),
    (0.35, 0.78, 0.49),
    (0.85, 0.35, 0.46),
    (0.62, 0.45, 0.92),
];

#[derive(Clone)]
pub struct DiskBar {
    drawing_area: gtk::DrawingArea,
    segments: Rc<RefCell<Vec<(String, u64)>>>,
}

impl DiskBar {
    pub fn new() -> Self {
        let drawing_area = gtk::DrawingArea::new();
        drawing_area.set_content_height(32);
        drawing_area.set_hexpand(true);

        let segments: Rc<RefCell<Vec<(String, u64)>>> = Rc::new(RefCell::new(Vec::new()));

        {
            let segments = segments.clone();
            drawing_area.set_draw_func(move |_area, cr, width, height| {
                let (width, height) = (width as f64, height as f64);
                let segments = segments.borrow();
                let total: u64 = segments.iter().map(|(_, size)| *size).sum();
                if total == 0 {
                    cr.set_source_rgba(1.0, 1.0, 1.0, 0.08);
                    cr.rectangle(0.0, 0.0, width, height);
                    let _ = cr.fill();
                    return;
                }

                let mut x = 0.0;
                for (i, (_, size)) in segments.iter().enumerate() {
                    let segment_width = width * (*size as f64 / total as f64);
                    let (r, g, b) = PALETTE[i % PALETTE.len()];
                    cr.set_source_rgb(r, g, b);
                    cr.rectangle(x, 0.0, segment_width, height);
                    let _ = cr.fill();
                    x += segment_width;
                }
            });
        }

        Self {
            drawing_area,
            segments,
        }
    }

    pub fn widget(&self) -> gtk::Widget {
        self.drawing_area.clone().upcast()
    }

    pub fn set_segments(&self, segments: Vec<(String, u64)>) {
        *self.segments.borrow_mut() = segments;
        self.drawing_area.queue_draw();
    }
}

impl Default for DiskBar {
    fn default() -> Self {
        Self::new()
    }
}
