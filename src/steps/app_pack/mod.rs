use crate::config::{AppCard, AppSelection, InstallConfig};
use crate::i18n::tr;
use crate::mx;
use crate::steps::card_layout::{
    CARD_SPACING, PAGE_MARGIN, add_column_breakpoint, column_min_width, grid_width,
};
use crate::steps::{Step, StepId, ValidityTracker};
use adw::prelude::*;
use std::cell::Cell;
use std::rc::Rc;

/// Step 9's cards are smaller than step 8's: there are nine of them, laid out
/// three rows deep, and three step-8-sized cards never fit the logical width
/// the kiosk guarantees (`nix/kiosk-module.nix` floors the logical size at
/// 1280x800 and ceilings the height at 1300, and libadwaita's navigation split
/// view takes 280 px of that for the step rail).
///
/// The cards carry no aspect ratio: their height comes from their content (a
/// fixed [`ICON_BOX_PX`] icon block plus the labels), which is what keeps the
/// `gtk::FlowBox` lines exactly as tall as the cards and therefore the row gap
/// equal to the column gap.
const CARD_MIN_WIDTH: i32 = 220;
const CARD_MAX_WIDTH: i32 = 420;

const TWO_COL_MIN: i32 = 560;
const THREE_COL_MIN: i32 = 800;

const _: () = assert!(TWO_COL_MIN >= column_min_width(2, CARD_MIN_WIDTH));
const _: () = assert!(THREE_COL_MIN >= column_min_width(3, CARD_MIN_WIDTH));

/// Card heading.
///
/// `pub(crate)` because `finish::summary` renders the same names — same
/// arrangement as `steps::desktop_environment::display_label`. `tr` is called
/// inside the match rather than on the returned value so the msgids are
/// literals `tests/po_coverage.rs` can extract.
pub(crate) fn card_title(card: AppCard) -> String {
    match card {
        AppCard::Base => tr("Base pack"),
        AppCard::OfficeSuite => tr("Office suite"),
        AppCard::Modeling3D => tr("3D modeling"),
        AppCard::VideoEditing => tr("Video editing"),
        AppCard::Graphics => tr("Drawing and image editing"),
        AppCard::Streaming => tr("Recording and streaming"),
        AppCard::Gaming => tr("Gaming"),
        AppCard::VirtualReality => tr("Virtual reality"),
        AppCard::SimHardware => tr("Simulation hardware"),
    }
}

/// One-line card subtitle, naming the software the card brings in.
fn card_description(card: AppCard) -> String {
    match card {
        AppCard::Base => tr("The essentials, ready to use"),
        AppCard::OfficeSuite => tr("Write documents, build spreadsheets and presentations"),
        AppCard::Modeling3D => tr("Model, animate and render in 3D with Blender"),
        AppCard::VideoEditing => tr("Kdenlive and DaVinci Resolve"),
        AppCard::Graphics => tr("GIMP, Krita and Inkscape"),
        AppCard::Streaming => tr("Record your screen and stream live with OBS Studio"),
        AppCard::Gaming => tr("Steam, Lutris, Heroic and UMU, with controller support"),
        AppCard::VirtualReality => tr("Stream VR games to a standalone headset with ALVR"),
        AppCard::SimHardware => {
            tr("Racing wheels and pedals from Logitech, Fanatec and Thrustmaster")
        }
    }
}

/// Label of one of a card's mutually exclusive editions, as shown on the radio
/// button inside the card.
///
/// `pub(crate)` because `finish::summary` names the picked edition too.
///
/// # Parameters
/// * `card` - the card the edition belongs to.
/// * `key` - the edition's `config::CardVariant::key`.
///
/// # Returns
/// The translated label. An unknown `(card, key)` pair falls back to the key
/// itself rather than panicking: a missing label is a cosmetic bug, and the
/// keys are developer-facing ASCII anyway.
pub(crate) fn variant_label(card: AppCard, key: &str) -> String {
    match (card, key) {
        (AppCard::VideoEditing, "free") => tr("Free"),
        (AppCard::VideoEditing, "studio") => tr("Studio"),
        _ => key.to_string(),
    }
}

/// What a screen reader reads as the card's description: its subtitle, plus
/// the picked edition for a card that offers several — the radio row is inside
/// the toggle button, so the toggle's own announcement has to carry it.
///
/// # Parameters
/// * `card` - the card.
/// * `variant` - index into `card.variants()`.
fn accessible_description(card: AppCard, variant: usize) -> String {
    let description = card_description(card);
    match card.variant_at(variant) {
        Some(edition) => format!("{description} — {}", variant_label(card, edition.key)),
        None => description,
    }
}

/// Third card line, only on the card whose contents are worth spelling out.
fn card_details(card: AppCard) -> Option<String> {
    match card {
        AppCard::Base => Some(tr(
            "Web browser, email, file manager, terminal, printing, PDF reader, image viewer, archive manager, text editor, media player, and the Flathub app store",
        )),
        _ => None,
    }
}

/// Icon-theme lookup size. GTK's icon-theme pipeline (unlike
/// `gtk::Picture::for_resource`, which rasterizes an SVG once at its
/// intrinsic `width`/`height` attribute and then stretches that fixed
/// bitmap) re-renders the source SVG at whatever pixel size is requested —
/// looking up generously above [`ICON_BOX_PX`] keeps the icon sharp on a
/// HiDPI panel, where the compositor scales the whole surface up.
const ICON_LOOKUP_PX: i32 = 256;

/// Side of a single-icon card's icon block, in logical pixels, whatever the
/// card's own width. A `gtk::Image` reports exactly its `pixel-size` in both
/// orientations, which is what keeps every card's requested height
/// independent of its width — see [`icon_widget_for`].
const ICON_BOX_PX: i32 = 96;

/// Side of one tile of a collage. Deliberately not `ICON_BOX_PX / 2`: at half
/// size the grouped icons read as clutter rather than as the applications they
/// name, so a collage is allowed to be wider and taller than a lone icon.
const ICON_TILE_PX: i32 = 72;

/// Gap between the tiles of a collage.
const ICON_COLLAGE_SPACING: i32 = 6;

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

/// One icon at a fixed displayed size.
///
/// # Parameters
/// * `display` - display whose icon theme resolves `name`.
/// * `name` - a `modulix-app-pack-*` alias from the gresource.
/// * `size_px` - the `pixel-size`, i.e. the widget's requested side.
fn icon_image(display: &gtk::gdk::Display, name: &str, size_px: i32) -> gtk::Image {
    let image = gtk::Image::from_paintable(Some(&lookup_icon_paintable(display, name)));
    image.set_pixel_size(size_px);
    image.set_halign(gtk::Align::Center);
    image.set_valign(gtk::Align::Center);
    image
}

/// Icons filling the card's icon block, as the `modulix-app-pack-*` aliases
/// registered in `data/resources.gresource.xml` — resolved out of the Papirus
/// icon theme (GPL-3.0) straight from the nixpkgs derivation at build time.
///
/// One to four names: [`icon_widget_for`] lays a single one out on its own and
/// the rest in a two-column grid, the odd third one spanning both columns so
/// it stays centred instead of leaving a hole. UMU is left out of the gaming
/// collage — Papirus gives it the generic Wine icon, which tells a user
/// nothing — and a gamepad stands in for it, since controller support is what
/// the card adds on top of the launchers.
fn icon_names_for(card: AppCard) -> &'static [&'static str] {
    match card {
        AppCard::Base => &[
            "modulix-app-pack-firefox",
            "modulix-app-pack-thunderbird",
            "modulix-app-pack-nautilus",
            "modulix-app-pack-printer",
        ],
        AppCard::OfficeSuite => &["modulix-app-pack-papers"],
        AppCard::Modeling3D => &["modulix-app-pack-blender"],
        AppCard::VideoEditing => &[
            "modulix-app-pack-kdenlive",
            "modulix-app-pack-davinci-resolve",
        ],
        AppCard::Graphics => &[
            "modulix-app-pack-gimp",
            "modulix-app-pack-krita",
            "modulix-app-pack-inkscape",
        ],
        AppCard::Streaming => &["modulix-app-pack-obs"],
        AppCard::Gaming => &[
            "modulix-app-pack-steam",
            "modulix-app-pack-lutris",
            "modulix-app-pack-heroic",
            "modulix-app-pack-gamepad",
        ],
        AppCard::VirtualReality => &["modulix-app-pack-vr"],
        AppCard::SimHardware => &["modulix-app-pack-sim-hardware"],
    }
}

/// Card icon block: a lone [`ICON_BOX_PX`] icon for a single-app card,
/// otherwise a two-column grid of [`ICON_TILE_PX`] tiles — a row of two, or
/// two rows with the third tile spanning both columns.
///
/// Sizing the block rather than letting it grow with the card is what makes
/// every card request the same height at any width, so the `gtk::FlowBox`
/// lines are exactly as tall as the cards and the gap between two rows is the
/// `row-spacing` and nothing else — a card letterboxed inside a taller line
/// reads as a wider vertical gap than horizontal one.
fn icon_widget_for(display: &gtk::gdk::Display, card: AppCard) -> gtk::Widget {
    let names = icon_names_for(card);
    if let [only] = names {
        return icon_image(display, only, ICON_BOX_PX).upcast();
    }
    let grid = gtk::Grid::builder()
        .row_spacing(ICON_COLLAGE_SPACING)
        .column_spacing(ICON_COLLAGE_SPACING)
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .build();
    let odd_last = names.len() % 2 == 1;
    for (index, name) in names.iter().enumerate() {
        let image = icon_image(display, name, ICON_TILE_PX);
        let last = index + 1 == names.len();
        if odd_last && last {
            grid.attach(&image, 0, (index / 2) as i32, 2, 1);
        } else {
            grid.attach(&image, (index % 2) as i32, (index / 2) as i32, 1, 1);
        }
    }
    grid.upcast()
}

/// Per-card widgets kept for `retranslate()` and for reading the selection
/// back in `commit()`.
///
/// The tick indicator is deliberately absent: it is driven entirely from the
/// `connect_toggled` handler that owns the only other reference to it.
struct CardWidgets {
    card: AppCard,
    button: gtk::ToggleButton,
    name: gtk::Label,
    description: gtk::Label,
    details: Option<gtk::Label>,
    /// Index into `card.variants()`, written by the radio handlers and read
    /// back by `commit`. Shared with those handlers, hence the `Rc`; always
    /// `AppCard::DEFAULT_VARIANT` for a card with no edition radios.
    variant: Rc<Cell<usize>>,
    /// The edition radios with their `config::CardVariant::key`, kept for
    /// `retranslate()`. Empty for a card with no edition choice.
    variant_buttons: Vec<(gtk::CheckButton, &'static str)>,
}

/// Applications step: a multi-select grid, three columns wide at any usable
/// window size, with the base pack ticked and everything else off.
///
/// Selection state lives in the toggle buttons themselves rather than in a
/// shadow `Cell`, so there is one source of truth; `commit` reads it back into
/// an [`AppSelection`]. The empty selection is legal and means a bare system,
/// which is why the step is never `Blocked`.
pub struct AppPackStep {
    widget: gtk::Widget,
    title_label: gtk::Label,
    footnote_label: gtk::Label,
    hint_label: gtk::Label,
    cards: Vec<CardWidgets>,
    validity: ValidityTracker,
}

impl Default for AppPackStep {
    fn default() -> Self {
        Self::new()
    }
}

impl AppPackStep {
    pub fn new() -> Self {
        let display =
            gtk::gdk::Display::default().expect("a display must exist by the time steps are built");

        let title_label = gtk::Label::builder()
            .label(tr(
                "The essential applications will be installed for each activity",
            ))
            .xalign(0.0)
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .css_classes(["title-2"])
            .build();

        let footnote_label = gtk::Label::builder()
            .label(tr("You can install or remove any of this afterwards"))
            .xalign(0.0)
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .css_classes(["dim-label"])
            .build();

        let hint_label = gtk::Label::builder()
            .label(tr(
                "Nothing selected — Modulix OS will install a bare system",
            ))
            .xalign(0.0)
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .css_classes(["dim-label", "caption"])
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
        let mut indicators = Vec::new();
        for card in AppCard::ALL {
            // The icon takes every pixel the text leaves and centres itself in
            // it, so it sits halfway between the top of the card and the top of
            // the text whatever the text's own height — the base pack's card
            // carries one line more than the others, and `homogeneous` makes
            // every card as tall as that one.
            let icon = icon_widget_for(&display, card);
            icon.set_vexpand(true);
            icon.set_valign(gtk::Align::Center);

            let title = card_title(card);
            let description = card_description(card);

            let name_label = gtk::Label::builder()
                .label(&title)
                .wrap(true)
                .wrap_mode(gtk::pango::WrapMode::WordChar)
                .justify(gtk::Justification::Center)
                .max_width_chars(24)
                .css_classes(["title-4"])
                .build();
            let desc_label = gtk::Label::builder()
                .label(&description)
                .wrap(true)
                .wrap_mode(gtk::pango::WrapMode::WordChar)
                .justify(gtk::Justification::Center)
                .max_width_chars(30)
                .css_classes(["dim-label"])
                .build();

            // Text block pinned to the bottom of the card: a non-expanding box
            // aligned to the end, so every spare pixel goes to the icon above.
            let text_box = gtk::Box::builder()
                .orientation(gtk::Orientation::Vertical)
                .spacing(8)
                .valign(gtk::Align::End)
                .margin_top(8)
                .build();
            text_box.append(&name_label);
            text_box.append(&desc_label);

            let details_label = card_details(card).map(|details| {
                let label = gtk::Label::builder()
                    .label(&details)
                    .wrap(true)
                    .wrap_mode(gtk::pango::WrapMode::WordChar)
                    .justify(gtk::Justification::Center)
                    .max_width_chars(30)
                    .css_classes(["dim-label", "caption"])
                    .build();
                text_box.append(&label);
                label
            });

            // Edition radios, last line of the text block: mutually
            // exclusive, so a `gtk::CheckButton` group rather than a second
            // set of toggles.
            //
            // They live *inside* the card's `gtk::ToggleButton`, and nesting
            // alone is not enough: the ToggleButton's own click gesture wins
            // the sequence, so a click meant for a radio only flipped the
            // card's tick and never reached the radio. Each radio therefore
            // carries a `gtk::GestureClick` in the **capture** phase — which
            // runs before any bubble-phase gesture on the way back up —
            // claiming the sequence to cancel the card's gesture, and driving
            // `set_active` itself since claiming also denies the radio's own
            // internal gesture. Keyboard activation is untouched: it never
            // goes through a click gesture.
            let variant = Rc::new(Cell::new(AppCard::DEFAULT_VARIANT));
            let mut variant_buttons: Vec<(gtk::CheckButton, &'static str)> = Vec::new();
            let editions = card.variants();
            if !editions.is_empty() {
                let row = gtk::Box::builder()
                    .orientation(gtk::Orientation::Horizontal)
                    .spacing(12)
                    .halign(gtk::Align::Center)
                    .margin_top(4)
                    .build();
                for (index, edition) in editions.iter().enumerate() {
                    let radio = gtk::CheckButton::builder()
                        .label(variant_label(card, edition.key))
                        .build();
                    if let Some((first, _)) = variant_buttons.first() {
                        radio.set_group(Some(first));
                    }
                    // The default edition is settled here, before any handler
                    // exists — the handlers below tick the card, which must
                    // not happen on the page's first frame.
                    radio.set_active(index == AppCard::DEFAULT_VARIANT);

                    let click = gtk::GestureClick::new();
                    click.set_propagation_phase(gtk::PropagationPhase::Capture);
                    let target = radio.clone();
                    click.connect_pressed(move |gesture, _, _, _| {
                        gesture.set_state(gtk::EventSequenceState::Claimed);
                        target.set_active(true);
                    });
                    radio.add_controller(click);

                    row.append(&radio);
                    variant_buttons.push((radio, edition.key));
                }
                text_box.append(&row);
            }

            let inner = gtk::Box::new(gtk::Orientation::Vertical, 0);
            inner.append(&icon);
            inner.append(&text_box);

            let card_button = gtk::ToggleButton::builder()
                .child(&inner)
                .css_classes(["card", "de-card", "flat"])
                .build();
            // The card's visual is an icon collage with no text of its own, so
            // the name a screen reader announces has to be set explicitly
            // rather than derived from the content.
            card_button.update_property(&[
                gtk::accessible::Property::Label(&title),
                gtk::accessible::Property::Description(&accessible_description(
                    card,
                    AppCard::DEFAULT_VARIANT,
                )),
            ]);

            // Picking an edition means wanting the card, so a radio ticks it —
            // one click instead of two, and a card left unticked with an
            // edition chosen would read as a selection that is not one.
            for (index, (radio, _)) in variant_buttons.iter().enumerate() {
                let variant = variant.clone();
                let button = card_button.clone();
                radio.connect_toggled(move |radio| {
                    if !radio.is_active() {
                        return;
                    }
                    variant.set(index);
                    button.set_active(true);
                    button.update_property(&[gtk::accessible::Property::Description(
                        &accessible_description(card, index),
                    )]);
                });
            }

            // Indicator only: `can_target(false)` lets the click through to the
            // toggle underneath, `focusable(false)` keeps it out of the Tab
            // chain, and the presentation role stops Orca announcing a second,
            // phantom check box on top of the toggle's own pressed state.
            let indicator = gtk::CheckButton::builder()
                .can_target(false)
                .focusable(false)
                .accessible_role(gtk::AccessibleRole::Presentation)
                .halign(gtk::Align::End)
                .valign(gtk::Align::Start)
                .margin_top(10)
                .margin_end(10)
                .build();

            // The card is the flow-box child itself, with no aspect frame
            // around it: a fixed-ratio frame letterboxes the card inside a
            // line sized by the tallest card, which reads as a vertical gap
            // wider than the horizontal one. With the card filling its cell,
            // both gaps are exactly `CARD_SPACING`.
            let overlay = gtk::Overlay::builder()
                .child(&card_button)
                .width_request(CARD_MIN_WIDTH)
                .hexpand(true)
                .build();
            overlay.add_overlay(&indicator);

            grid.append(&overlay);
            indicators.push(indicator);
            cards.push(CardWidgets {
                card,
                button: card_button,
                name: name_label,
                description: desc_label,
                details: details_label,
                variant,
                variant_buttons,
            });
        }

        let buttons: Rc<Vec<gtk::ToggleButton>> =
            Rc::new(cards.iter().map(|c| c.button.clone()).collect());
        for (card, indicator) in cards.iter().zip(indicators) {
            let buttons = buttons.clone();
            let hint = hint_label.clone();
            card.button.connect_toggled(move |button| {
                let selected = button.is_active();
                indicator.set_active(selected);
                if selected {
                    button.add_css_class("de-card-selected");
                } else {
                    button.remove_css_class("de-card-selected");
                }
                hint.set_visible(!buttons.iter().any(gtk::prelude::ToggleButtonExt::is_active));
            });
        }

        // Applied after the handlers are connected so the default selection
        // settles the indicator, the outline and the hint through the same path
        // a click takes.
        for card in &cards {
            card.button.set_active(card.card == AppCard::Base);
        }
        hint_label.set_visible(!buttons.iter().any(gtk::prelude::ToggleButtonExt::is_active));

        let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
        content.set_margin_top(PAGE_MARGIN);
        content.set_margin_bottom(PAGE_MARGIN);
        content.set_margin_start(PAGE_MARGIN);
        content.set_margin_end(PAGE_MARGIN);
        content.append(&title_label);
        content.append(&footnote_label);
        content.append(&grid);
        content.append(&hint_label);

        let clamp = adw::Clamp::builder()
            .maximum_size(grid_width(1, CARD_MAX_WIDTH))
            .tightening_threshold(CARD_MIN_WIDTH)
            .child(&content)
            .build();

        let bin = adw::BreakpointBin::builder()
            .width_request(CARD_MIN_WIDTH + 2 * PAGE_MARGIN)
            .height_request(600)
            .child(&clamp)
            .build();

        add_column_breakpoint(&bin, &grid, &clamp, TWO_COL_MIN, 2, CARD_MAX_WIDTH);
        add_column_breakpoint(&bin, &grid, &clamp, THREE_COL_MIN, 3, CARD_MAX_WIDTH);

        let scrolled = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&bin)
            .build();

        Self {
            widget: scrolled.upcast(),
            title_label,
            footnote_label,
            hint_label,
            cards,
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
        cfg.apps = AppSelection::from_choices(
            self.cards
                .iter()
                .filter(|c| c.button.is_active())
                .map(|c| (c.card, c.variant.get())),
        );
        Ok(())
    }

    fn retranslate(&self) {
        self.title_label.set_label(&tr(
            "The essential applications will be installed for each activity",
        ));
        self.footnote_label
            .set_label(&tr("You can install or remove any of this afterwards"));
        self.hint_label.set_label(&tr(
            "Nothing selected — Modulix OS will install a bare system",
        ));
        for card in &self.cards {
            let title = card_title(card.card);
            let description = card_description(card.card);
            card.name.set_label(&title);
            card.description.set_label(&description);
            if let (Some(label), Some(details)) = (&card.details, card_details(card.card)) {
                label.set_label(&details);
            }
            for (radio, key) in &card.variant_buttons {
                radio.set_label(Some(&variant_label(card.card, key)));
            }
            card.button.update_property(&[
                gtk::accessible::Property::Label(&title),
                gtk::accessible::Property::Description(&accessible_description(
                    card.card,
                    card.variant.get(),
                )),
            ]);
        }
    }
}
