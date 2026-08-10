use crate::config::{DesktopEnvironment, InstallConfig};
use crate::i18n::tr;
use crate::mx;
use crate::steps::{Step, StepId, ValidityTracker};
use adw::prelude::*;
use std::cell::Cell;
use std::rc::Rc;

const CARD_MIN_WIDTH: i32 = 260;
const CARD_MAX_WIDTH: i32 = 560;
const CARD_SPACING: i32 = 12;
const PAGE_MARGIN: i32 = 24;

/// Width of an N-card row at the card's max width (N cards + (N-1) gaps) —
/// the `adw::Clamp` ceiling for that column count.
const fn grid_width(columns: i32) -> i32 {
    columns * CARD_MAX_WIDTH + (columns - 1) * CARD_SPACING
}

/// `BreakpointBin` width (not the `Clamp`'s) at which 2/3 columns kick in.
/// `THREE_COL_MIN` is picked so a 1920px 16:9 fullscreen — minus the
/// `NavigationSplitView` rail (`app.rs`, ~280px) — stays at 2 columns; a
/// 2560px ultrawide clears it into 3.
const TWO_COL_MIN: i32 = 800;
const THREE_COL_MIN: i32 = 1780;

/// msgid, not the DE's proper name — see [`DesktopEnvironment::de_name`] for
/// that. Also used by `finish::summary` to render the same label.
pub(crate) fn display_label(de: DesktopEnvironment) -> &'static str {
    match de {
        DesktopEnvironment::Gnome => "Modern",
        DesktopEnvironment::Plasma => "Classic",
        DesktopEnvironment::Lxqt => "Lightweight",
    }
}

fn description_for(de: DesktopEnvironment) -> &'static str {
    match de {
        DesktopEnvironment::Gnome => "A clean, modern desktop focused on simplicity",
        DesktopEnvironment::Plasma => "A powerful, customizable desktop with a traditional layout",
        DesktopEnvironment::Lxqt => "A lightweight desktop for older or low-resource hardware",
    }
}

pub struct DesktopEnvironmentStep {
    widget: gtk::Widget,
    title_label: gtk::Label,
    cards: Vec<(DesktopEnvironment, gtk::Label, gtk::Label)>,
    selected: Rc<Cell<DesktopEnvironment>>,
    validity: ValidityTracker,
}

impl DesktopEnvironmentStep {
    pub fn new() -> Self {
        let selected = Rc::new(Cell::new(DesktopEnvironment::default()));

        let title_label = gtk::Label::builder()
            .label(tr("Desktop environment"))
            .xalign(0.0)
            .css_classes(["title-2"])
            .build();

        let grid = gtk::FlowBox::builder()
            .selection_mode(gtk::SelectionMode::Single)
            .activate_on_single_click(true)
            .homogeneous(true)
            .min_children_per_line(1)
            .max_children_per_line(1)
            .row_spacing(CARD_SPACING as u32)
            .column_spacing(CARD_SPACING as u32)
            .build();

        let mut cards = Vec::new();
        for de in DesktopEnvironment::ALL {
            let picture = gtk::Picture::for_resource(de.screenshot_resource());
            picture.set_content_fit(gtk::ContentFit::Cover);
            picture.set_can_shrink(true);
            let frame = gtk::AspectFrame::builder()
                .ratio(16.0 / 9.0)
                .obey_child(false)
                .child(&picture)
                .build();

            let name_label = gtk::Label::builder()
                .label(format!("{} ({})", tr(display_label(de)), de.de_name()))
                .wrap(true)
                .wrap_mode(gtk::pango::WrapMode::WordChar)
                .justify(gtk::Justification::Center)
                .max_width_chars(28)
                .css_classes(["title-4"])
                .build();
            let desc_label = gtk::Label::builder()
                .label(tr(description_for(de)))
                .wrap(true)
                .wrap_mode(gtk::pango::WrapMode::WordChar)
                .justify(gtk::Justification::Center)
                .max_width_chars(34)
                .css_classes(["dim-label"])
                .build();

            let card = gtk::Box::new(gtk::Orientation::Vertical, 12);
            card.add_css_class("card");
            card.add_css_class("de-card");
            card.set_width_request(CARD_MIN_WIDTH);
            card.set_hexpand(true);
            card.append(&frame);
            card.append(&name_label);
            card.append(&desc_label);

            grid.append(&card);
            cards.push((de, name_label, desc_label));
        }

        if let Some(default_index) = cards
            .iter()
            .position(|(de, ..)| *de == DesktopEnvironment::default())
            && let Some(child) = grid.child_at_index(default_index as i32)
        {
            grid.select_child(&child);
        }

        {
            let selected = selected.clone();
            grid.connect_selected_children_changed(move |grid| {
                if let Some(child) = grid.selected_children().first() {
                    let index = child.index();
                    if let Some(de) = DesktopEnvironment::ALL.get(index as usize) {
                        selected.set(*de);
                    }
                }
            });
        }

        let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
        content.set_margin_top(PAGE_MARGIN);
        content.set_margin_bottom(PAGE_MARGIN);
        content.set_margin_start(PAGE_MARGIN);
        content.set_margin_end(PAGE_MARGIN);
        content.append(&title_label);
        content.append(&grid);

        let clamp = adw::Clamp::builder()
            .maximum_size(grid_width(1))
            .tightening_threshold(CARD_MIN_WIDTH)
            .child(&content)
            .build();

        let bin = adw::BreakpointBin::builder()
            .width_request(CARD_MIN_WIDTH + 2 * PAGE_MARGIN)
            .height_request(600)
            .child(&clamp)
            .build();

        let bp2 = adw::Breakpoint::new(adw::BreakpointCondition::new_length(
            adw::BreakpointConditionLengthType::MinWidth,
            TWO_COL_MIN as f64,
            adw::LengthUnit::Px,
        ));
        bp2.add_setter(&grid, "min-children-per-line", Some(&2u32.to_value()));
        bp2.add_setter(&grid, "max-children-per-line", Some(&2u32.to_value()));
        bp2.add_setter(&clamp, "maximum-size", Some(&grid_width(2).to_value()));
        bin.add_breakpoint(bp2);

        let bp3 = adw::Breakpoint::new(adw::BreakpointCondition::new_length(
            adw::BreakpointConditionLengthType::MinWidth,
            THREE_COL_MIN as f64,
            adw::LengthUnit::Px,
        ));
        bp3.add_setter(&grid, "min-children-per-line", Some(&3u32.to_value()));
        bp3.add_setter(&grid, "max-children-per-line", Some(&3u32.to_value()));
        bp3.add_setter(&clamp, "maximum-size", Some(&grid_width(3).to_value()));
        bin.add_breakpoint(bp3);

        let scrolled = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&bin)
            .build();

        Self {
            widget: scrolled.upcast(),
            title_label,
            cards,
            selected,
            validity: ValidityTracker::ready(),
        }
    }
}

impl Default for DesktopEnvironmentStep {
    fn default() -> Self {
        Self::new()
    }
}

impl Step for DesktopEnvironmentStep {
    fn id(&self) -> StepId {
        StepId::DesktopEnvironment
    }

    fn title(&self) -> String {
        tr("Desktop")
    }

    fn icon_name(&self) -> &'static str {
        "desktop-symbolic"
    }

    fn widget(&self) -> gtk::Widget {
        self.widget.clone()
    }

    fn validity(&self) -> ValidityTracker {
        self.validity.clone()
    }

    fn commit(&self, cfg: &mut InstallConfig) -> mx::Result<()> {
        cfg.desktop_environment = self.selected.get();
        Ok(())
    }

    fn retranslate(&self) {
        self.title_label.set_label(&tr("Desktop environment"));
        for (de, name_label, desc_label) in &self.cards {
            name_label.set_label(&format!("{} ({})", tr(display_label(*de)), de.de_name()));
            desc_label.set_label(&tr(description_for(*de)));
        }
    }
}
