use crate::config::{DesktopEnvironment, InstallConfig};
use crate::i18n::tr;
use crate::mx;
use crate::steps::{AdvanceHook, Step, StepId, ValidityTracker};
use adw::prelude::*;
use std::cell::Cell;
use std::rc::Rc;

pub(crate) const CARD_MIN_WIDTH: i32 = 300;
pub(crate) const CARD_MAX_WIDTH: i32 = 560;
pub(crate) const CARD_SPACING: i32 = 12;
pub(crate) const PAGE_MARGIN: i32 = 24;

/// Width of an N-card row at the card's max width (N cards + (N-1) gaps) —
/// the `adw::Clamp` ceiling for that column count.
pub(crate) const fn grid_width(columns: i32) -> i32 {
    columns * CARD_MAX_WIDTH + (columns - 1) * CARD_SPACING
}

const TWO_COL_MIN: i32 = 800;
const FOUR_COL_MIN: i32 = 2360;

/// msgid, not the DE's proper name — see [`DesktopEnvironment::de_name`] for
/// that. Also used by `finish::summary` to render the same label.
pub(crate) fn display_label(de: DesktopEnvironment) -> &'static str {
    match de {
        DesktopEnvironment::Gnome => "Modern",
        DesktopEnvironment::Plasma => "Classic",
        DesktopEnvironment::Xfce => "Light and ready to roll",
        DesktopEnvironment::Lxqt => "Light and customizable",
    }
}

fn description_for(de: DesktopEnvironment) -> &'static str {
    match de {
        DesktopEnvironment::Gnome => "A clean, modern desktop focused on simplicity",
        DesktopEnvironment::Plasma => "A powerful, customizable desktop with a traditional layout",
        DesktopEnvironment::Xfce => "A light, ready-to-use desktop, complete out of the box",
        DesktopEnvironment::Lxqt => {
            "A very light, tweakable desktop for older or low-resource hardware"
        }
    }
}

fn refresh_selection(buttons: &[(DesktopEnvironment, gtk::Button)], selected: DesktopEnvironment) {
    for (de, button) in buttons {
        if *de == selected {
            button.add_css_class("de-card-selected");
        } else {
            button.remove_css_class("de-card-selected");
        }
    }
}

/// Opens the zoomed detail dialog for `de`: a large preview + the "Choose
/// this desktop environment" action. Rebuilt from scratch on every click, so
/// its labels are always in the current language — no `retranslate()` hook
/// needed for it.
fn present_zoom(
    parent: &impl IsA<gtk::Widget>,
    de: DesktopEnvironment,
    selected: Rc<Cell<DesktopEnvironment>>,
    buttons: Rc<Vec<(DesktopEnvironment, gtk::Button)>>,
    advance: AdvanceHook,
) {
    let screenshots = de.screenshots();

    let carousel = adw::Carousel::builder()
        .allow_scroll_wheel(true)
        .vexpand(true)
        .build();
    for resource in screenshots {
        let picture = gtk::Picture::for_resource(resource);
        picture.set_content_fit(gtk::ContentFit::Contain);
        picture.set_can_shrink(true);
        let frame = gtk::AspectFrame::builder()
            .ratio(16.0 / 9.0)
            .obey_child(false)
            .child(&picture)
            .vexpand(true)
            .build();
        carousel.append(&frame);
    }

    let prev_button = gtk::Button::from_icon_name("go-previous-symbolic");
    prev_button.add_css_class("flat");
    prev_button.add_css_class("circular");
    prev_button.set_valign(gtk::Align::Center);
    prev_button.set_vexpand(false);
    let next_button = gtk::Button::from_icon_name("go-next-symbolic");
    next_button.add_css_class("flat");
    next_button.add_css_class("circular");
    next_button.set_valign(gtk::Align::Center);
    next_button.set_vexpand(false);
    prev_button.set_visible(screenshots.len() > 1);
    next_button.set_visible(screenshots.len() > 1);
    {
        let carousel = carousel.clone();
        prev_button.connect_clicked(move |_| {
            let pos = carousel.position().round() as u32;
            if pos > 0 {
                let page = carousel.nth_page(pos - 1);
                carousel.scroll_to(&page, true);
            }
        });
    }
    {
        let carousel = carousel.clone();
        next_button.connect_clicked(move |_| {
            let pos = carousel.position().round() as u32;
            if pos + 1 < carousel.n_pages() {
                let page = carousel.nth_page(pos + 1);
                carousel.scroll_to(&page, true);
            }
        });
    }

    let carousel_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    carousel_row.set_halign(gtk::Align::Center);
    carousel_row.append(&prev_button);
    carousel_row.append(&carousel);
    carousel_row.append(&next_button);

    let dots = adw::CarouselIndicatorDots::builder()
        .carousel(&carousel)
        .halign(gtk::Align::Center)
        .build();
    dots.set_visible(screenshots.len() > 1);

    let carousel_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
    carousel_box.append(&carousel_row);
    carousel_box.append(&dots);

    let name_label = gtk::Label::builder()
        .label(format!("{} ({})", tr(display_label(de)), de.de_name()))
        .xalign(0.0)
        .wrap(true)
        .css_classes(["title-2"])
        .build();
    let desc_label = gtk::Label::builder()
        .label(tr(description_for(de)))
        .xalign(0.0)
        .wrap(true)
        .css_classes(["dim-label"])
        .build();

    let text_box = gtk::Box::new(gtk::Orientation::Vertical, 4);
    text_box.set_hexpand(true);
    text_box.set_valign(gtk::Align::Center);
    text_box.append(&name_label);
    text_box.append(&desc_label);

    let back_button = gtk::Button::from_icon_name("go-previous-symbolic");
    back_button.set_tooltip_text(Some(&tr("Back")));

    let choose_button = gtk::Button::builder()
        .label(tr("Choose this desktop environment"))
        .valign(gtk::Align::Center)
        .css_classes(["suggested-action", "pill"])
        .build();

    let info_row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    info_row.append(&text_box);
    info_row.append(&choose_button);

    let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
    content.set_margin_top(24);
    content.set_margin_bottom(24);
    content.set_margin_start(24);
    content.set_margin_end(24);
    content.append(&carousel_box);
    content.append(&info_row);

    let header = adw::HeaderBar::new();
    header.set_show_end_title_buttons(false);
    header.pack_start(&back_button);

    let toolbar_view = adw::ToolbarView::new();
    toolbar_view.add_top_bar(&header);
    toolbar_view.set_content(Some(&content));

    let dialog = adw::Dialog::builder()
        .title(de.de_name())
        .content_width(1300)
        .content_height(931)
        .child(&toolbar_view)
        .build();

    {
        let dialog = dialog.clone();
        back_button.connect_clicked(move |_| {
            dialog.close();
        });
    }
    {
        let dialog = dialog.clone();
        choose_button.connect_clicked(move |_| {
            selected.set(de);
            refresh_selection(&buttons, de);
            dialog.close();
            (advance.borrow())();
        });
    }

    dialog.present(Some(parent));
}

pub struct DesktopEnvironmentStep {
    widget: gtk::Widget,
    title_label: gtk::Label,
    cards: Vec<(DesktopEnvironment, gtk::Label, gtk::Label)>,
    selected: Rc<Cell<DesktopEnvironment>>,
    validity: ValidityTracker,
}

impl DesktopEnvironmentStep {
    pub fn new(advance: AdvanceHook) -> Self {
        let de_count = DesktopEnvironment::ALL.len() as i32;
        let selected = Rc::new(Cell::new(DesktopEnvironment::default()));

        let title_label = gtk::Label::builder()
            .label(tr("Desktop environment"))
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
        for de in DesktopEnvironment::ALL {
            let picture = gtk::Picture::for_resource(de.screenshot_resource());
            picture.set_content_fit(gtk::ContentFit::Cover);
            picture.set_can_shrink(true);
            let img_frame = gtk::AspectFrame::builder()
                .ratio(16.0 / 9.0)
                .obey_child(false)
                .child(&picture)
                .vexpand(true)
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
                .lines(3)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .max_width_chars(34)
                .css_classes(["dim-label"])
                .build();

            let inner = gtk::Box::new(gtk::Orientation::Vertical, 8);
            inner.append(&img_frame);
            inner.append(&name_label);
            inner.append(&desc_label);

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
            cards.push((de, name_label, desc_label));
            buttons.push((de, card_button));
        }

        let buttons = Rc::new(buttons);
        refresh_selection(&buttons, selected.get());

        for (de, button) in buttons.iter() {
            let de = *de;
            let selected = selected.clone();
            let buttons = buttons.clone();
            let advance = advance.clone();
            button.connect_clicked(move |button| {
                present_zoom(
                    button,
                    de,
                    selected.clone(),
                    buttons.clone(),
                    advance.clone(),
                );
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

        let add_bp = |min_width: i32, columns: i32| {
            let bp = adw::Breakpoint::new(adw::BreakpointCondition::new_length(
                adw::BreakpointConditionLengthType::MinWidth,
                min_width as f64,
                adw::LengthUnit::Px,
            ));
            bp.add_setter(
                &grid,
                "min-children-per-line",
                Some(&(columns as u32).to_value()),
            );
            bp.add_setter(
                &grid,
                "max-children-per-line",
                Some(&(columns as u32).to_value()),
            );
            bp.add_setter(
                &clamp,
                "maximum-size",
                Some(&grid_width(columns).to_value()),
            );
            bin.add_breakpoint(bp);
        };
        if de_count >= 2 {
            add_bp(TWO_COL_MIN, 2);
        }
        if de_count >= 3 {
            add_bp(FOUR_COL_MIN, de_count.min(4));
        }

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

    fn shows_next(&self) -> bool {
        false
    }

    fn retranslate(&self) {
        self.title_label.set_label(&tr("Desktop environment"));
        for (de, name_label, desc_label) in &self.cards {
            name_label.set_label(&format!("{} ({})", tr(display_label(*de)), de.de_name()));
            desc_label.set_label(&tr(description_for(*de)));
        }
    }
}
