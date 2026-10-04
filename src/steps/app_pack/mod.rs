use crate::config::{AppPack, InstallConfig};
use crate::i18n::tr;
use crate::mx;
use crate::steps::desktop_environment::{CARD_MIN_WIDTH, CARD_SPACING, PAGE_MARGIN, grid_width};
use crate::steps::{AdvanceHook, Step, StepId, ValidityTracker};
use adw::prelude::*;
use std::cell::Cell;
use std::rc::Rc;

const TWO_COL_MIN: i32 = 800;

fn title_for(pack: AppPack) -> &'static str {
    match pack {
        AppPack::None => "No applications",
        AppPack::Base => "Base pack",
    }
}

fn description_for(pack: AppPack) -> &'static str {
    match pack {
        AppPack::None => "Start from a bare system and install everything yourself",
        AppPack::Base => "The essentials, ready to use",
    }
}

fn details_for(pack: AppPack) -> Option<&'static str> {
    match pack {
        AppPack::None => None,
        AppPack::Base => Some(
            "Web browser, file manager, terminal, printing, PDF reader, image viewer, archive manager, text editor, media player, and the Flathub app store",
        ),
    }
}

fn note_for(pack: AppPack) -> Option<&'static str> {
    match pack {
        AppPack::None => None,
        AppPack::Base => Some("You can uninstall any of these afterwards"),
    }
}

/// Icon-theme lookup size. GTK's icon-theme pipeline (unlike
/// `gtk::Picture::for_resource`, which rasterizes an SVG once at its
/// intrinsic `width`/`height` attribute and then stretches that fixed
/// bitmap) re-renders the source SVG at whatever pixel size is requested —
/// asking for something generously larger than any card will ever display
/// keeps the icon sharp even inside a maximized window.
const ICON_LOOKUP_PX: i32 = 256;

fn lookup_icon_paintable(display: &gtk::gdk::Display, name: &str) -> gtk::IconPaintable {
    gtk::IconTheme::for_display(display).lookup_icon(
        name,
        &[],
        ICON_LOOKUP_PX,
        1,
        gtk::TextDirection::None,
        gtk::IconLookupFlags::empty(),
    )
}

fn icon_picture(display: &gtk::gdk::Display, name: &str) -> gtk::Picture {
    let picture = gtk::Picture::new();
    picture.set_paintable(Some(&lookup_icon_paintable(display, name)));
    picture.set_content_fit(gtk::ContentFit::Contain);
    picture
}

/// Card icon block, wrapped by the caller in a 1:1 `adw::AspectFrame` so it
/// grows with the card instead of staying pinned at a fixed pixel size.
/// `None` reuses the GNOME Software (app store) icon — go get your own
/// apps; `Base` is a 2x2 collage of the bundled apps' own icons (Firefox,
/// Nautilus, Papers, a printer) instead of one generic glyph. Icons are
/// looked up from the Papirus icon theme (GPL-3.0), resolved straight out of
/// the nixpkgs derivation at build time — see `data/resources.gresource.xml`.
fn icon_widget_for(display: &gtk::gdk::Display, pack: AppPack) -> gtk::Widget {
    match pack {
        AppPack::None => icon_picture(display, "modulix-app-pack-store").upcast(),
        AppPack::Base => {
            let grid = gtk::Grid::builder()
                .row_spacing(4)
                .column_spacing(4)
                .vexpand(true)
                .hexpand(true)
                .build();
            for (col, row, name) in [
                (0, 0, "modulix-app-pack-firefox"),
                (1, 0, "modulix-app-pack-nautilus"),
                (0, 1, "modulix-app-pack-papers"),
                (1, 1, "modulix-app-pack-printer"),
            ] {
                let picture = icon_picture(display, name);
                picture.set_vexpand(true);
                picture.set_hexpand(true);
                grid.attach(&picture, col, row, 1, 1);
            }
            grid.upcast()
        }
    }
}

fn refresh_selection(buttons: &[(AppPack, gtk::Button)], selected: AppPack) {
    for (pack, button) in buttons {
        if *pack == selected {
            button.add_css_class("de-card-selected");
        } else {
            button.remove_css_class("de-card-selected");
        }
    }
}

/// Per-card cached labels: pack, title, description, details, note —
/// re-set by `retranslate()` on a language change.
type CardLabels = (
    AppPack,
    gtk::Label,
    gtk::Label,
    Option<gtk::Label>,
    Option<gtk::Label>,
);

pub struct AppPackStep {
    widget: gtk::Widget,
    title_label: gtk::Label,
    cards: Vec<CardLabels>,
    selected: Rc<Cell<AppPack>>,
    validity: ValidityTracker,
}

impl AppPackStep {
    pub fn new(advance: AdvanceHook) -> Self {
        let display =
            gtk::gdk::Display::default().expect("a display must exist by the time steps are built");
        let selected = Rc::new(Cell::new(AppPack::default()));

        let title_label = gtk::Label::builder()
            .label(tr("Application pack"))
            .xalign(0.0)
            .css_classes(["title-2"])
            .build();

        let grid = gtk::FlowBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .homogeneous(true)
            .min_children_per_line(1)
            .max_children_per_line(1)
            .row_spacing(CARD_SPACING as u32)
            .column_spacing(CARD_SPACING as u32)
            .build();

        let mut cards = Vec::new();
        let mut buttons = Vec::new();
        for pack in AppPack::ALL {
            let icon = icon_widget_for(&display, pack);
            let icon_frame = gtk::AspectFrame::builder()
                .ratio(1.0)
                .obey_child(false)
                .vexpand(true)
                .hexpand(true)
                .child(&icon)
                .build();

            let name_label = gtk::Label::builder()
                .label(tr(title_for(pack)))
                .wrap(true)
                .wrap_mode(gtk::pango::WrapMode::WordChar)
                .justify(gtk::Justification::Center)
                .max_width_chars(28)
                .css_classes(["title-4"])
                .build();
            let desc_label = gtk::Label::builder()
                .label(tr(description_for(pack)))
                .wrap(true)
                .wrap_mode(gtk::pango::WrapMode::WordChar)
                .justify(gtk::Justification::Center)
                .max_width_chars(34)
                .css_classes(["dim-label"])
                .build();

            let inner = gtk::Box::new(gtk::Orientation::Vertical, 8);
            inner.append(&icon_frame);
            inner.append(&name_label);
            inner.append(&desc_label);

            let details_label = details_for(pack).map(|details| {
                let label = gtk::Label::builder()
                    .label(tr(details))
                    .wrap(true)
                    .wrap_mode(gtk::pango::WrapMode::WordChar)
                    .justify(gtk::Justification::Center)
                    .max_width_chars(34)
                    .css_classes(["dim-label", "caption"])
                    .build();
                inner.append(&label);
                label
            });

            let note_label = note_for(pack).map(|note| {
                let label = gtk::Label::builder()
                    .label(tr(note))
                    .wrap(true)
                    .wrap_mode(gtk::pango::WrapMode::WordChar)
                    .justify(gtk::Justification::Center)
                    .max_width_chars(34)
                    .css_classes(["dim-label", "caption"])
                    .build();
                inner.append(&label);
                label
            });

            let card_button = gtk::Button::builder()
                .child(&inner)
                .css_classes(["card", "de-card", "flat"])
                .build();

            let outer = gtk::AspectFrame::builder()
                .ratio(4.0 / 3.0)
                .obey_child(false)
                .child(&card_button)
                .build();
            outer.set_width_request(CARD_MIN_WIDTH);
            outer.set_hexpand(true);

            grid.append(&outer);
            cards.push((pack, name_label, desc_label, details_label, note_label));
            buttons.push((pack, card_button));
        }

        let buttons = Rc::new(buttons);
        refresh_selection(&buttons, selected.get());

        for (pack, button) in buttons.iter() {
            let pack = *pack;
            let selected = selected.clone();
            let buttons = buttons.clone();
            let advance = advance.clone();
            button.connect_clicked(move |_| {
                selected.set(pack);
                refresh_selection(&buttons, pack);
                (advance.borrow())();
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

impl Step for AppPackStep {
    fn id(&self) -> StepId {
        StepId::AppPack
    }

    fn title(&self) -> String {
        tr("Applications")
    }

    fn icon_name(&self) -> &'static str {
        "apps-symbolic"
    }

    fn widget(&self) -> gtk::Widget {
        self.widget.clone()
    }

    fn validity(&self) -> ValidityTracker {
        self.validity.clone()
    }

    fn commit(&self, cfg: &mut InstallConfig) -> mx::Result<()> {
        cfg.app_pack = self.selected.get();
        Ok(())
    }

    fn shows_next(&self) -> bool {
        false
    }

    fn retranslate(&self) {
        self.title_label.set_label(&tr("Application pack"));
        for (pack, name_label, desc_label, details_label, note_label) in &self.cards {
            name_label.set_label(&tr(title_for(*pack)));
            desc_label.set_label(&tr(description_for(*pack)));
            if let (Some(label), Some(details)) = (details_label, details_for(*pack)) {
                label.set_label(&tr(details));
            }
            if let (Some(label), Some(note)) = (note_label, note_for(*pack)) {
                label.set_label(&tr(note));
            }
        }
    }
}
