//! Proportional bar showing a disk's partition layout (partitioning step).
//! Colored **by role**, not by index — the ESP stays blue even if the order
//! of segments changes between the "before" and "after" bars.

use crate::i18n::tr;
use gtk::prelude::*;
use std::cell::{Cell, RefCell};
use std::f64::consts::{FRAC_PI_2, PI};
use std::rc::Rc;

const CORNER_RADIUS: f64 = 8.0;
const SEGMENT_GAP: f64 = 2.0;
pub const MIN_SEGMENT_PX: f64 = 4.0;

/// Binary (GiB/MiB) formatting — every disk/partition size in this codebase
/// comes from a byte count computed in binary units (1 MiB = 2^20 bytes), so
/// formatting with a decimal (1e9) divisor as the old code did would show a
/// misleading number (a "250 GiB" fake disk read as "268 GB").
pub fn human_bytes(bytes: u64) -> String {
    const GIB: f64 = (1u64 << 30) as f64;
    const MIB: f64 = (1u64 << 20) as f64;
    let bytes_f = bytes as f64;
    if bytes_f >= GIB {
        format!("{:.1} GiB", bytes_f / GIB)
    } else {
        format!("{:.0} MiB", bytes_f / MIB)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SegmentRole {
    Esp,
    Root,
    Swap,
    Home,
    Windows,
    Other,
    Free,
}

impl SegmentRole {
    pub fn name(self) -> String {
        match self {
            SegmentRole::Esp => tr("EFI System Partition"),
            SegmentRole::Root => tr("Root"),
            SegmentRole::Swap => tr("Swap"),
            SegmentRole::Home => tr("Home"),
            SegmentRole::Windows => tr("Windows"),
            SegmentRole::Other => tr("Other"),
            SegmentRole::Free => tr("Free space"),
        }
    }

    fn color(self) -> (f64, f64, f64) {
        match self {
            SegmentRole::Esp => (0.31, 0.60, 0.96),
            SegmentRole::Root => (0.35, 0.78, 0.49),
            SegmentRole::Swap => (0.98, 0.68, 0.14),
            SegmentRole::Home => (0.25, 0.75, 0.73),
            SegmentRole::Windows => (0.62, 0.45, 0.92),
            SegmentRole::Other => (0.85, 0.35, 0.46),
            SegmentRole::Free => (0.6, 0.6, 0.6),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct DiskBarSegment {
    pub label: String,
    pub size_bytes: u64,
    pub role: SegmentRole,
    /// True for the root segment when LUKS2 encryption is enabled — drawn
    /// with a hatched overlay on top of the role color, see `draw_func`.
    pub encrypted: bool,
    /// Bytes in use inside the filesystem, when known — drawn as a
    /// one-direction diagonal-stripe overlay clipped to the used sub-rect
    /// (`draw_used_overlay`). `None` draws the segment flat.
    pub used_bytes: Option<u64>,
}

/// Label for the whole bar's `AccessibleRole::Img` — narrated as a single
/// sentence since the bar itself paints proportional colored rectangles with
/// no text of their own. Extended over the original (label + size only) to
/// also speak each segment's role, since "512 MiB" alone doesn't say this is
/// the boot partition.
pub fn accessible_summary(segments: &[DiskBarSegment]) -> String {
    if segments.is_empty() {
        return tr("No partitions");
    }
    segments
        .iter()
        .map(|s| {
            let mut parts = vec![s.role.name(), human_bytes(s.size_bytes)];
            if let Some(used) = s.used_bytes {
                parts.push(format!("{} {}", human_bytes(used), tr("used")));
            }
            if s.encrypted {
                parts.push(tr("encrypted"));
            }
            format!("{} ({})", s.label, parts.join(", "))
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// `(x, width)` per segment for a bar of the given pixel `width`, guaranteeing
/// at least [`MIN_SEGMENT_PX`] per segment (a 512 MiB ESP on a 1 TB disk would
/// otherwise round to a sub-pixel sliver) by clamping undersized segments and
/// redistributing the remaining width proportionally among the rest.
pub fn segment_rects(width: f64, segs: &[DiskBarSegment]) -> Vec<(f64, f64)> {
    let n = segs.len();
    if n == 0 || width <= 0.0 {
        return Vec::new();
    }
    let total_size: u64 = segs.iter().map(|s| s.size_bytes).sum();
    if total_size == 0 {
        let w = width / n as f64;
        return (0..n).map(|i| (w * i as f64, w)).collect();
    }

    let mut widths = vec![0.0f64; n];
    let mut fixed = vec![false; n];
    loop {
        let fixed_width: f64 = (0..n).filter(|&i| fixed[i]).map(|i| widths[i]).sum();
        let remaining_width = (width - fixed_width).max(0.0);
        let remaining_size: u64 = (0..n)
            .filter(|&i| !fixed[i])
            .map(|i| segs[i].size_bytes)
            .sum();
        if remaining_size == 0 {
            break;
        }

        let mut changed = false;
        for i in 0..n {
            if fixed[i] {
                continue;
            }
            let w = remaining_width * segs[i].size_bytes as f64 / remaining_size as f64;
            if w < MIN_SEGMENT_PX && remaining_width > 0.0 {
                widths[i] = MIN_SEGMENT_PX;
                fixed[i] = true;
                changed = true;
            } else {
                widths[i] = w;
            }
        }
        if !changed {
            break;
        }
    }

    let mut x = 0.0;
    let mut rects = Vec::with_capacity(n);
    for w in widths {
        rects.push((x, w));
        x += w;
    }
    rects
}

/// Width in pixels of the used-space sub-rect within a `sw`-pixel-wide
/// segment representing `size` bytes, `used` of which are in use — pure so
/// `draw_used_overlay`'s geometry can be unit-tested without a `cairo`
/// context. Clamped to `[0, sw]`: a stale/inconsistent `used > size` must
/// never overflow the segment it's drawn into.
pub fn used_width(sw: f64, used: u64, size: u64) -> f64 {
    if size == 0 {
        return 0.0;
    }
    (sw * (used as f64 / size as f64)).clamp(0.0, sw)
}

fn rounded_rect_path(cr: &gtk::cairo::Context, x: f64, y: f64, w: f64, h: f64, r: f64) {
    let r = r.min(w / 2.0).min(h / 2.0).max(0.0);
    cr.new_sub_path();
    cr.arc(x + w - r, y + r, r, -FRAC_PI_2, 0.0);
    cr.arc(x + w - r, y + h - r, r, 0.0, FRAC_PI_2);
    cr.arc(x + r, y + h - r, r, FRAC_PI_2, PI);
    cr.arc(x + r, y + r, r, PI, 3.0 * FRAC_PI_2);
    cr.close_path();
}

/// Diagonal-stripe fill for `Free` segments — distinguishes unallocated
/// space from a solid-color role at a glance, including for users who can't
/// rely on hue alone.
fn draw_hatched(cr: &gtk::cairo::Context, x: f64, y: f64, w: f64, h: f64) {
    let _ = cr.save();
    cr.rectangle(x, y, w, h);
    cr.clip();
    cr.set_source_rgba(1.0, 1.0, 1.0, 0.06);
    cr.rectangle(x, y, w, h);
    let _ = cr.fill();
    cr.set_source_rgba(1.0, 1.0, 1.0, 0.18);
    cr.set_line_width(2.0);
    let step = 8.0;
    let mut offset = -h;
    while offset < w {
        cr.move_to(x + offset, y + h);
        cr.line_to(x + offset + h, y);
        offset += step;
    }
    let _ = cr.stroke();
    let _ = cr.restore();
}

/// One-direction diagonal stripes over the used-space sub-rect of an
/// already-filled segment — distinguishes used space from free space within
/// a role color without hiding it, mirroring `draw_hatched`'s stripe style
/// but confined to `used_width(w, ...)` instead of the whole segment.
fn draw_used_overlay(cr: &gtk::cairo::Context, x: f64, y: f64, w: f64, h: f64) {
    let _ = cr.save();
    cr.rectangle(x, y, w, h);
    cr.clip();
    cr.set_source_rgba(0.0, 0.0, 0.0, 0.22);
    cr.set_line_width(1.5);
    let step = 6.0;
    let mut offset = -h;
    while offset < w {
        cr.move_to(x + offset, y + h);
        cr.line_to(x + offset + h, y);
        offset += step;
    }
    let _ = cr.stroke();
    let _ = cr.restore();
}

/// Cross-hatching (both diagonal directions) + a light liseré (outline) over
/// an already-filled segment — marks a LUKS2-encrypted segment without
/// hiding its role color, unlike `draw_hatched` which stands in for a color
/// entirely (`Free`). Two stroke passes in opposite directions distinguish
/// this from `draw_used_overlay`'s one-direction stripes at a glance.
fn draw_encrypted_overlay(cr: &gtk::cairo::Context, x: f64, y: f64, w: f64, h: f64) {
    let _ = cr.save();
    cr.rectangle(x, y, w, h);
    cr.clip();
    cr.set_source_rgba(0.0, 0.0, 0.0, 0.28);
    cr.set_line_width(1.2);
    let step = 5.0;
    let mut offset = -h;
    while offset < w {
        cr.move_to(x + offset, y + h);
        cr.line_to(x + offset + h, y);
        offset += step;
    }
    let _ = cr.stroke();
    offset = -h;
    while offset < w {
        cr.move_to(x + offset, y);
        cr.line_to(x + offset + h, y + h);
        offset += step;
    }
    let _ = cr.stroke();
    cr.set_source_rgba(1.0, 1.0, 1.0, 0.5);
    cr.set_line_width(1.0);
    cr.rectangle(x + 0.5, y + 0.5, (w - 1.0).max(0.0), (h - 1.0).max(0.0));
    let _ = cr.stroke();
    let _ = cr.restore();
}

#[derive(Clone)]
pub struct DiskBar {
    container: gtk::Box,
    drawing_area: gtk::DrawingArea,
    legend: gtk::FlowBox,
    segments: Rc<RefCell<Vec<DiskBarSegment>>>,
    high_contrast: Rc<Cell<bool>>,
}

impl DiskBar {
    pub fn new() -> Self {
        let drawing_area = gtk::DrawingArea::builder()
            .content_height(44)
            .hexpand(true)
            .accessible_role(gtk::AccessibleRole::Img)
            .build();
        drawing_area.update_property(&[gtk::accessible::Property::Label(&accessible_summary(&[]))]);
        drawing_area.set_has_tooltip(true);

        let segments: Rc<RefCell<Vec<DiskBarSegment>>> = Rc::new(RefCell::new(Vec::new()));
        let high_contrast = Rc::new(Cell::new(false));

        {
            let segments = segments.clone();
            let high_contrast = high_contrast.clone();
            drawing_area.set_draw_func(move |_area, cr, width, height| {
                let (width, height) = (width as f64, height as f64);
                let segments = segments.borrow();

                rounded_rect_path(cr, 0.0, 0.0, width, height, CORNER_RADIUS);
                let _ = cr.save();
                cr.clip_preserve();

                if segments.is_empty() {
                    cr.set_source_rgba(1.0, 1.0, 1.0, 0.08);
                    cr.rectangle(0.0, 0.0, width, height);
                    let _ = cr.fill();
                    let _ = cr.restore();
                    return;
                }

                cr.set_source_rgba(0.0, 0.0, 0.0, 0.25);
                cr.rectangle(0.0, 0.0, width, height);
                let _ = cr.fill();

                let rects = segment_rects(width, &segments);
                let n = rects.len();
                for (i, (seg, (x, w))) in segments.iter().zip(rects.iter()).enumerate() {
                    let inset_left = if i > 0 { SEGMENT_GAP / 2.0 } else { 0.0 };
                    let inset_right = if i + 1 < n { SEGMENT_GAP / 2.0 } else { 0.0 };
                    let sx = x + inset_left;
                    let sw = (w - inset_left - inset_right).max(0.0);

                    if seg.role == SegmentRole::Free {
                        draw_hatched(cr, sx, 0.0, sw, height);
                    } else {
                        let (r, g, b) = seg.role.color();
                        cr.set_source_rgb(r, g, b);
                        cr.rectangle(sx, 0.0, sw, height);
                        let _ = cr.fill();
                        if let Some(used) = seg.used_bytes {
                            let uw = used_width(sw, used, seg.size_bytes);
                            draw_used_overlay(cr, sx, 0.0, uw, height);
                        }
                        if seg.encrypted {
                            draw_encrypted_overlay(cr, sx, 0.0, sw, height);
                        }
                    }

                    if high_contrast.get() {
                        cr.set_source_rgb(1.0, 1.0, 1.0);
                        cr.set_line_width(1.0);
                        cr.rectangle(sx + 0.5, 0.5, (sw - 1.0).max(0.0), height - 1.0);
                        let _ = cr.stroke();
                    }
                }
                let _ = cr.restore();
            });
        }

        {
            let segments = segments.clone();
            drawing_area.connect_query_tooltip(move |area, x, _y, _keyboard, tooltip| {
                let segments = segments.borrow();
                if segments.is_empty() {
                    return false;
                }
                let width = area.width() as f64;
                let rects = segment_rects(width, &segments);
                let Some(idx) = rects.iter().position(|(rx, rw)| {
                    let x = x as f64;
                    x >= *rx && x < *rx + *rw
                }) else {
                    return false;
                };
                let seg = &segments[idx];
                let mut parts = vec![seg.role.name()];
                if let Some(used) = seg.used_bytes {
                    parts.push(format!("{} {}", human_bytes(used), tr("used")));
                }
                if seg.encrypted {
                    parts.push(tr("encrypted"));
                }
                tooltip.set_text(Some(&format!(
                    "{} — {} ({})",
                    seg.label,
                    human_bytes(seg.size_bytes),
                    parts.join(", ")
                )));
                true
            });
        }

        let legend = gtk::FlowBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .row_spacing(4)
            .column_spacing(12)
            .homogeneous(false)
            .build();

        let container = gtk::Box::new(gtk::Orientation::Vertical, 6);
        container.append(&drawing_area);
        container.append(&legend);

        Self {
            container,
            drawing_area,
            legend,
            segments,
            high_contrast,
        }
    }

    pub fn widget(&self) -> gtk::Widget {
        self.container.clone().upcast()
    }

    pub fn set_high_contrast(&self, enabled: bool) {
        self.high_contrast.set(enabled);
        self.drawing_area.queue_draw();
    }

    pub fn set_segments(&self, segments: Vec<DiskBarSegment>) {
        self.drawing_area
            .update_property(&[gtk::accessible::Property::Label(&accessible_summary(
                &segments,
            ))]);
        *self.segments.borrow_mut() = segments;
        self.rebuild_legend();
        self.drawing_area.queue_draw();
    }

    fn rebuild_legend(&self) {
        while let Some(child) = self.legend.first_child() {
            self.legend.remove(&child);
        }
        for seg in self.segments.borrow().iter() {
            self.legend.append(&legend_entry(seg));
        }
    }

    /// Re-derives role names (translated UI chrome) after a language change.
    /// Segment `label`s themselves are caller-provided data (a partition's
    /// on-disk label, a role's English msgid already resolved when the plan
    /// was built) and aren't re-fetched here.
    pub fn retranslate(&self) {
        self.drawing_area
            .update_property(&[gtk::accessible::Property::Label(&accessible_summary(
                &self.segments.borrow(),
            ))]);
        self.rebuild_legend();
    }
}

fn legend_entry(seg: &DiskBarSegment) -> gtk::Box {
    let (r, g, b) = seg.role.color();
    let swatch = gtk::DrawingArea::builder()
        .content_width(10)
        .content_height(10)
        .valign(gtk::Align::Center)
        .build();
    swatch.set_draw_func(move |_area, cr, w, h| {
        rounded_rect_path(cr, 0.0, 0.0, w as f64, h as f64, 2.0);
        cr.set_source_rgb(r, g, b);
        let _ = cr.fill();
    });

    let mut parts = Vec::new();
    if let Some(used) = seg.used_bytes {
        parts.push(format!("{} {}", human_bytes(used), tr("used")));
    }
    if seg.encrypted {
        parts.push(tr("encrypted"));
    }
    let label_text = if parts.is_empty() {
        format!("{} — {}", seg.label, human_bytes(seg.size_bytes))
    } else {
        format!(
            "{} — {} ({})",
            seg.label,
            human_bytes(seg.size_bytes),
            parts.join(", ")
        )
    };
    let label = gtk::Label::new(Some(&label_text));
    label.add_css_class("caption");

    let entry = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    entry.append(&swatch);
    entry.append(&label);
    entry
}

impl Default for DiskBar {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(label: &str, size: u64, role: SegmentRole) -> DiskBarSegment {
        DiskBarSegment {
            label: label.to_string(),
            size_bytes: size,
            role,
            encrypted: false,
            used_bytes: None,
        }
    }

    #[test]
    fn accessible_summary_empty_is_no_partitions() {
        assert_eq!(accessible_summary(&[]), "No partitions");
    }

    #[test]
    fn accessible_summary_lists_label_role_and_size() {
        const GIB: u64 = 1 << 30;
        const MIB: u64 = 1 << 20;
        let segments = vec![
            seg("sda1", 100 * GIB, SegmentRole::Root),
            seg("Free space", 500 * MIB, SegmentRole::Free),
        ];
        assert_eq!(
            accessible_summary(&segments),
            "sda1 (Root, 100.0 GiB), Free space (Free space, 500 MiB)"
        );
    }

    #[test]
    fn accessible_summary_mentions_encryption() {
        const GIB: u64 = 1 << 30;
        let mut root = seg("sda2", 100 * GIB, SegmentRole::Root);
        root.encrypted = true;
        assert_eq!(
            accessible_summary(&[root]),
            "sda2 (Root, 100.0 GiB, encrypted)"
        );
    }

    #[test]
    fn human_bytes_uses_binary_units() {
        assert_eq!(human_bytes(1 << 30), "1.0 GiB");
        assert_eq!(human_bytes((1 << 30) + (1 << 29)), "1.5 GiB");
        assert_eq!(human_bytes(512 * (1 << 20)), "512 MiB");
    }

    #[test]
    fn segment_rects_sum_equals_total_width() {
        let segs = vec![
            seg("a", 512 * (1 << 20), SegmentRole::Esp),
            seg("b", 100 * (1 << 30), SegmentRole::Root),
            seg("c", 8 * (1 << 30), SegmentRole::Swap),
        ];
        let rects = segment_rects(576.0, &segs);
        let total: f64 = rects.iter().map(|(_, w)| w).sum();
        assert!((total - 576.0).abs() < 0.01);
    }

    #[test]
    fn segment_rects_proportional_for_equal_sizes() {
        let segs = vec![
            seg("a", 100, SegmentRole::Root),
            seg("b", 100, SegmentRole::Swap),
        ];
        let rects = segment_rects(400.0, &segs);
        assert!((rects[0].1 - 200.0).abs() < 0.01);
        assert!((rects[1].1 - 200.0).abs() < 0.01);
    }

    #[test]
    fn segment_rects_enforces_minimum_width() {
        // A tiny 1 MiB ESP next to a 999 GiB root would round to a
        // sub-pixel sliver without the minimum-width floor.
        let segs = vec![
            seg("esp", 1 << 20, SegmentRole::Esp),
            seg("root", 999 * (1 << 30), SegmentRole::Root),
        ];
        let rects = segment_rects(576.0, &segs);
        assert!(rects[0].1 >= MIN_SEGMENT_PX - 0.01);
    }

    #[test]
    fn segment_rects_empty_input_is_empty() {
        assert!(segment_rects(576.0, &[]).is_empty());
    }

    #[test]
    fn used_width_is_proportional() {
        assert!((used_width(100.0, 50, 200) - 25.0).abs() < 0.01);
        assert!((used_width(100.0, 200, 200) - 100.0).abs() < 0.01);
    }

    #[test]
    fn used_width_clamps_to_segment_width() {
        // Stale/inconsistent `used > size` must never overflow the segment.
        assert_eq!(used_width(100.0, 300, 200), 100.0);
    }

    #[test]
    fn used_width_zero_size_is_zero() {
        assert_eq!(used_width(100.0, 0, 0), 0.0);
    }

    #[test]
    fn accessible_summary_mentions_used_bytes() {
        const GIB: u64 = 1 << 30;
        let mut windows = seg("Windows", 400 * GIB, SegmentRole::Windows);
        windows.used_bytes = Some(210 * GIB);
        assert_eq!(
            accessible_summary(&[windows]),
            "Windows (Windows, 400.0 GiB, 210.0 GiB used)"
        );
    }
}
