//! Clickable equirectangular timezone map (step 3). The background is a real
//! NASA Visible Earth texture (`data/images/earth.jpg`, see the README next
//! to it) so continent outlines are genuine, not hand-drawn; the projection
//! math and nearest-point lookup are pure functions so they're unit-testable
//! without GTK.

use crate::backend::locale::TimezoneEntry;
use gtk::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;

const EARTH_RESOURCE: &str = "/org/modulix/installer/images/earth.jpg";

/// Longitude/latitude (degrees) -> pixel position in a `width x height` equirectangular canvas.
pub fn project(lat: f64, lon: f64, width: f64, height: f64) -> (f64, f64) {
    let x = (lon + 180.0) / 360.0 * width;
    let y = (90.0 - lat) / 180.0 * height;
    (x, y)
}

/// Inverse of [`project`]: pixel position -> (latitude, longitude) in degrees.
pub fn unproject(x: f64, y: f64, width: f64, height: f64) -> (f64, f64) {
    let lon = (x / width) * 360.0 - 180.0;
    let lat = 90.0 - (y / height) * 180.0;
    (lat, lon)
}

/// Nearest entry by squared equirectangular distance — good enough for
/// "which zone did the user click near," not meant for great-circle accuracy.
pub fn nearest_entry(entries: &[TimezoneEntry], lat: f64, lon: f64) -> Option<&TimezoneEntry> {
    entries.iter().min_by(|a, b| {
        let da = (a.latitude - lat).powi(2) + (a.longitude - lon).powi(2);
        let db = (b.latitude - lat).powi(2) + (b.longitude - lon).powi(2);
        da.total_cmp(&db)
    })
}

type SelectCallback = Box<dyn Fn(&TimezoneEntry)>;

/// Cheap to clone — every field is an `Rc`/refcounted GObject handle onto the
/// same underlying widget and state, which is what lets several signal
/// closures each hold their own handle to the same map.
#[derive(Clone)]
pub struct TimezoneMap {
    overlay: gtk::Overlay,
    /// Transparent, drawn over the earth picture: only paints the dots, and
    /// owns the click gesture (its own background is never painted).
    drawing_area: gtk::DrawingArea,
    entries: Rc<RefCell<Vec<TimezoneEntry>>>,
    selected: Rc<RefCell<Option<usize>>>,
    on_select: Rc<RefCell<SelectCallback>>,
}

impl TimezoneMap {
    pub fn new() -> Self {
        let picture = gtk::Picture::for_resource(EARTH_RESOURCE);
        picture.set_content_fit(gtk::ContentFit::Fill);
        picture.set_can_shrink(true);
        picture.set_size_request(640, 320);

        let drawing_area = gtk::DrawingArea::new();
        drawing_area.set_content_width(640);
        drawing_area.set_content_height(320);

        let overlay = gtk::Overlay::new();
        overlay.set_child(Some(&picture));
        overlay.add_overlay(&drawing_area);

        let entries: Rc<RefCell<Vec<TimezoneEntry>>> = Rc::new(RefCell::new(Vec::new()));
        let selected: Rc<RefCell<Option<usize>>> = Rc::new(RefCell::new(None));
        let on_select: Rc<RefCell<SelectCallback>> = Rc::new(RefCell::new(Box::new(|_| {})));

        {
            let entries = entries.clone();
            let selected = selected.clone();
            drawing_area.set_draw_func(move |_area, cr, width, height| {
                let (width, height) = (width as f64, height as f64);

                for (i, entry) in entries.borrow().iter().enumerate() {
                    let (x, y) = project(entry.latitude, entry.longitude, width, height);
                    let is_selected = *selected.borrow() == Some(i);

                    // A dark halo first so a dot stays legible over both
                    // bright (ice/desert) and dark (ocean) parts of the photo.
                    cr.set_source_rgba(0.0, 0.0, 0.0, 0.55);
                    cr.arc(
                        x,
                        y,
                        if is_selected { 6.5 } else { 4.5 },
                        0.0,
                        std::f64::consts::TAU,
                    );
                    let _ = cr.fill();

                    if is_selected {
                        cr.set_source_rgb(0.98, 0.68, 0.14);
                        cr.arc(x, y, 5.0, 0.0, std::f64::consts::TAU);
                    } else {
                        cr.set_source_rgba(0.4, 0.85, 1.0, 0.95);
                        cr.arc(x, y, 3.0, 0.0, std::f64::consts::TAU);
                    }
                    let _ = cr.fill();
                }
            });
        }

        let gesture = gtk::GestureClick::new();
        {
            let entries = entries.clone();
            let selected = selected.clone();
            let on_select = on_select.clone();
            let drawing_area_for_click = drawing_area.clone();
            gesture.connect_pressed(move |_gesture, _n_press, x, y| {
                let width = drawing_area_for_click.width() as f64;
                let height = drawing_area_for_click.height() as f64;
                if width <= 0.0 || height <= 0.0 {
                    return;
                }
                let (lat, lon) = unproject(x, y, width, height);
                let entries_ref = entries.borrow();
                if let Some(entry) = nearest_entry(&entries_ref, lat, lon) {
                    let idx = entries_ref
                        .iter()
                        .position(|e| e.name == entry.name)
                        .unwrap();
                    *selected.borrow_mut() = Some(idx);
                    (on_select.borrow())(entry);
                    drawing_area_for_click.queue_draw();
                }
            });
        }
        drawing_area.add_controller(gesture);

        Self {
            overlay,
            drawing_area,
            entries,
            selected,
            on_select,
        }
    }

    pub fn widget(&self) -> gtk::Widget {
        self.overlay.clone().upcast()
    }

    pub fn set_entries(&self, entries: Vec<TimezoneEntry>) {
        *self.entries.borrow_mut() = entries;
        self.drawing_area.queue_draw();
    }

    pub fn select_by_name(&self, name: &str) {
        let idx = self.entries.borrow().iter().position(|e| e.name == name);
        *self.selected.borrow_mut() = idx;
        self.drawing_area.queue_draw();
    }

    pub fn connect_selected<F: Fn(&TimezoneEntry) + 'static>(&self, f: F) {
        *self.on_select.borrow_mut() = Box::new(f);
    }
}

impl Default for TimezoneMap {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_unproject_round_trip() {
        let (x, y) = project(48.8667, 2.3333, 480.0, 240.0);
        let (lat, lon) = unproject(x, y, 480.0, 240.0);
        assert!((lat - 48.8667).abs() < 0.01);
        assert!((lon - 2.3333).abs() < 0.01);
    }

    #[test]
    fn origin_projects_to_center() {
        let (x, y) = project(0.0, 0.0, 480.0, 240.0);
        assert!((x - 240.0).abs() < 0.001);
        assert!((y - 120.0).abs() < 0.001);
    }

    #[test]
    fn nearest_entry_picks_closest() {
        let entries = vec![
            TimezoneEntry {
                name: "Europe/Paris".into(),
                latitude: 48.8667,
                longitude: 2.3333,
            },
            TimezoneEntry {
                name: "Asia/Tokyo".into(),
                latitude: 35.6833,
                longitude: 139.75,
            },
        ];
        let found = nearest_entry(&entries, 48.0, 2.0).unwrap();
        assert_eq!(found.name, "Europe/Paris");
    }

    #[test]
    fn nearest_entry_empty_is_none() {
        assert!(nearest_entry(&[], 0.0, 0.0).is_none());
    }
}
