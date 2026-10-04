use crate::backend::Backends;
use crate::backend::locale::KeyboardLayout;
use crate::backend::locale::keyboard_default::layout_for_locale;
use crate::bridge;
use crate::config::InstallConfig;
use crate::i18n::{tr, tr_xkb};
use crate::mx;
use crate::steps::{Step, StepId, ValidityTracker};
use crate::widgets::{enable_string_search, size_dropdown_to_widest};
use adw::prelude::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

const NO_VARIANT: &str = "";

/// Applies a layout/variant pair to the running session. Shared because both
/// the dropdown handlers and the language hook need it.
type ApplyLayout = Rc<dyn Fn(String, String)>;

/// Reacts to a layout becoming the selected one: refreshes the variant list
/// and applies the layout. Takes the index into the (sorted) layout vector.
type LayoutChanged = Rc<dyn Fn(usize)>;

pub struct KeyboardStep {
    widget: gtk::Widget,
    group: adw::PreferencesGroup,
    layout_row: adw::ActionRow,
    variant_row: adw::ActionRow,
    test_row: adw::ActionRow,
    error_banner: adw::Banner,
    layout_dropdown: gtk::DropDown,
    variant_dropdown: gtk::DropDown,
    /// Sorted by *displayed* name, so the index the dropdown reports indexes
    /// straight into this vector. Re-sorted on every language change.
    layouts: Rc<RefCell<Vec<KeyboardLayout>>>,
    selected_layout: Rc<RefCell<String>>,
    selected_variant: Rc<RefCell<String>>,
    /// True while a model is being swapped under a dropdown: the
    /// `notify::selected` that fires then carries an index into the *old*
    /// model and must be ignored.
    populating: Rc<Cell<bool>>,
    on_layout_changed: LayoutChanged,
    /// Set once the user picks a layout by hand — after that, a language
    /// change no longer overrides their choice.
    user_picked_layout: Rc<Cell<bool>>,
    /// Last locale handed over by the language step while the catalog was
    /// still loading; applied as soon as the layouts arrive.
    pending_locale: Rc<RefCell<Option<String>>>,
    validity: ValidityTracker,
}

/// Orders layouts by the name the user actually sees, which depends on the
/// current language — `evdev.xml`'s own order is neither alphabetical nor
/// meaningful.
fn sort_layouts(layouts: &mut [KeyboardLayout]) {
    layouts.sort_by_key(|layout| tr_xkb(&layout.description));
}

/// Rebuilds a layout dropdown's model with localized, sorted names.
///
/// * `dropdown` - the layout dropdown.
/// * `layouts` - layout catalog, already sorted by [`sort_layouts`].
/// * `code` - layout code to leave selected; falls back to the first entry.
/// * `populating` - guard flag, held for the duration of the swap.
///
/// # Post-conditions
/// The dropdown's selection points at `code` when the catalog holds it, and no
/// `notify::selected` emitted during the swap is treated as a user action.
fn fill_layout_dropdown(
    dropdown: &gtk::DropDown,
    layouts: &[KeyboardLayout],
    code: &str,
    populating: &Cell<bool>,
) {
    let names: Vec<String> = layouts
        .iter()
        .map(|layout| tr_xkb(&layout.description))
        .collect();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let model = gtk::StringList::new(&refs);
    let index = layouts
        .iter()
        .position(|layout| layout.code == code)
        .unwrap_or(0) as u32;

    populating.set(true);
    size_dropdown_to_widest(dropdown, &model);
    dropdown.set_model(Some(&model));
    dropdown.set_selected(index);
    populating.set(false);
}

/// Rebuilds a variant dropdown's model for `layout`.
///
/// * `dropdown` - the variant dropdown.
/// * `layout` - owning layout, or `None` while the catalog is still empty.
/// * `code` - variant code to leave selected; `NO_VARIANT` means the leading
///   "Default" entry.
/// * `populating` - guard flag, held for the duration of the swap.
///
/// # Post-conditions
/// Index 0 is always the "Default" sentinel, so index `n` maps to
/// `layout.variants[n - 1]`.
fn fill_variant_dropdown(
    dropdown: &gtk::DropDown,
    layout: Option<&KeyboardLayout>,
    code: &str,
    populating: &Cell<bool>,
) {
    let mut names = vec![tr("Default")];
    if let Some(layout) = layout {
        names.extend(
            layout
                .variants
                .iter()
                .map(|variant| tr_xkb(&variant.description)),
        );
    }
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let model = gtk::StringList::new(&refs);
    let index = layout
        .filter(|_| code != NO_VARIANT)
        .and_then(|layout| {
            layout
                .variants
                .iter()
                .position(|variant| variant.code == code)
        })
        .map(|position| position as u32 + 1)
        .unwrap_or(0);

    populating.set(true);
    size_dropdown_to_widest(dropdown, &model);
    dropdown.set_model(Some(&model));
    dropdown.set_selected(index);
    populating.set(false);
}

impl KeyboardStep {
    pub fn new(backends: &Backends, runtime: &tokio::runtime::Handle) -> Self {
        let layouts: Rc<RefCell<Vec<KeyboardLayout>>> = Rc::new(RefCell::new(Vec::new()));
        let selected_layout = Rc::new(RefCell::new("us".to_string()));
        let selected_variant = Rc::new(RefCell::new(NO_VARIANT.to_string()));
        let populating = Rc::new(Cell::new(false));
        let user_picked_layout = Rc::new(Cell::new(false));
        let pending_locale: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));

        let layout_dropdown = gtk::DropDown::from_strings(&["English (US)"]);
        enable_string_search(&layout_dropdown);
        let layout_row = adw::ActionRow::builder().title(tr("Layout")).build();
        layout_row.add_suffix(&layout_dropdown);

        let variant_dropdown = gtk::DropDown::from_strings(&[tr("Default").as_str()]);
        enable_string_search(&variant_dropdown);
        let variant_row = adw::ActionRow::builder().title(tr("Variant")).build();
        variant_row.add_suffix(&variant_dropdown);

        let test_entry = gtk::Entry::builder()
            .placeholder_text(tr("Type here to test your layout"))
            .build();
        let test_row = adw::ActionRow::builder().title(tr("Typing test")).build();
        test_row.add_suffix(&test_entry);
        test_row.set_activatable_widget(Some(&test_entry));

        let group = adw::PreferencesGroup::builder()
            .title(tr("Keyboard"))
            .build();
        group.add(&layout_row);
        group.add(&variant_row);
        group.add(&test_row);

        let page = adw::PreferencesPage::new();
        page.add(&group);
        page.set_vexpand(true);

        // A layout that fails to apply used to be swallowed silently, which is
        // exactly what hid the fact that `setxkbmap` never worked under the
        // Wayland kiosk session.
        let error_banner = adw::Banner::builder().revealed(false).build();

        let container = gtk::Box::new(gtk::Orientation::Vertical, 0);
        container.append(&error_banner);
        container.append(&page);

        let apply_layout: ApplyLayout = Rc::new({
            let backend = backends.locale.clone();
            let runtime = runtime.clone();
            let error_banner = error_banner.clone();
            move |layout: String, variant: String| {
                let backend = backend.clone();
                let error_banner = error_banner.clone();
                bridge::spawn(
                    &runtime,
                    async move { backend.apply_keyboard_layout(&layout, &variant).await },
                    move |result| match result {
                        Ok(()) => error_banner.set_revealed(false),
                        Err(e) => {
                            eprintln!("failed to apply the keyboard layout: {e}");
                            error_banner.set_title(&tr(
                                "This layout could not be applied to the running session.",
                            ));
                            error_banner.set_revealed(true);
                        }
                    },
                );
            }
        });

        // One place where "the selected layout changed" is handled, shared by
        // the dropdown and by the language hook — the latter has to run the
        // same side effects (variant list, live apply) as a click would.
        let on_layout_changed: LayoutChanged = Rc::new({
            let layouts = layouts.clone();
            let selected_layout = selected_layout.clone();
            let selected_variant = selected_variant.clone();
            let variant_dropdown = variant_dropdown.clone();
            let populating = populating.clone();
            let apply_layout = apply_layout.clone();
            move |index: usize| {
                let layouts_ref = layouts.borrow();
                let Some(layout) = layouts_ref.get(index) else {
                    return;
                };
                *selected_layout.borrow_mut() = layout.code.clone();
                *selected_variant.borrow_mut() = NO_VARIANT.to_string();
                fill_variant_dropdown(&variant_dropdown, Some(layout), NO_VARIANT, &populating);
                apply_layout(layout.code.clone(), NO_VARIANT.to_string());
            }
        });

        {
            let locale_backend = backends.locale.clone();
            let layouts = layouts.clone();
            let layout_dropdown = layout_dropdown.clone();
            let selected_layout = selected_layout.clone();
            let populating = populating.clone();
            let pending_locale = pending_locale.clone();
            let on_layout_changed = on_layout_changed.clone();
            bridge::spawn(
                runtime,
                async move { locale_backend.list_keyboard_layouts().await },
                move |result| {
                    let Ok(mut loaded) = result else { return };
                    if loaded.is_empty() {
                        return;
                    }
                    sort_layouts(&mut loaded);
                    // A language picked while this request was in flight still
                    // decides the default — otherwise the pre-selection is
                    // silently lost on a slow `evdev.xml` parse.
                    let wanted = pending_locale
                        .borrow()
                        .as_deref()
                        .and_then(|locale| layout_for_locale(locale, &loaded))
                        .unwrap_or_else(|| selected_layout.borrow().clone());
                    let index = loaded
                        .iter()
                        .position(|layout| layout.code == wanted)
                        .unwrap_or(0);
                    fill_layout_dropdown(&layout_dropdown, &loaded, &wanted, &populating);
                    *layouts.borrow_mut() = loaded;
                    on_layout_changed(index);
                },
            );
        }

        {
            let populating = populating.clone();
            let user_picked_layout = user_picked_layout.clone();
            let on_layout_changed = on_layout_changed.clone();
            layout_dropdown.connect_selected_notify(move |dropdown| {
                if populating.get() {
                    return;
                }
                user_picked_layout.set(true);
                on_layout_changed(dropdown.selected() as usize);
            });
        }

        {
            let layouts = layouts.clone();
            let selected_layout = selected_layout.clone();
            let selected_variant = selected_variant.clone();
            let populating = populating.clone();
            let apply_layout = apply_layout.clone();
            variant_dropdown.connect_selected_notify(move |dropdown| {
                if populating.get() {
                    return;
                }
                let index = dropdown.selected() as usize;
                let layouts_ref = layouts.borrow();
                let Some(layout) = layouts_ref
                    .iter()
                    .find(|l| l.code == *selected_layout.borrow())
                else {
                    return;
                };
                let variant_code = if index == 0 {
                    NO_VARIANT.to_string()
                } else {
                    layout
                        .variants
                        .get(index - 1)
                        .map(|variant| variant.code.clone())
                        .unwrap_or_default()
                };
                *selected_variant.borrow_mut() = variant_code.clone();
                apply_layout(layout.code.clone(), variant_code);
            });
        }

        Self {
            widget: container.upcast(),
            group,
            error_banner,
            layout_row,
            variant_row,
            test_row,
            layout_dropdown,
            variant_dropdown,
            layouts,
            selected_layout,
            selected_variant,
            populating,
            on_layout_changed,
            user_picked_layout,
            pending_locale,
            validity: ValidityTracker::ready(),
        }
    }

    /// Builds the callback the language step fires after every locale change.
    ///
    /// # Post-conditions
    /// The returned closure pre-selects the layout matching the new locale (see
    /// [`layout_for_locale`]) and applies it to the running session — unless
    /// the user has already picked a layout by hand, in which case it only
    /// records the locale and changes nothing.
    pub fn locale_hook(&self) -> Box<dyn Fn(&str)> {
        let layouts = self.layouts.clone();
        let layout_dropdown = self.layout_dropdown.clone();
        let populating = self.populating.clone();
        let user_picked_layout = self.user_picked_layout.clone();
        let pending_locale = self.pending_locale.clone();
        let on_layout_changed = self.on_layout_changed.clone();

        Box::new(move |locale: &str| {
            *pending_locale.borrow_mut() = Some(locale.to_string());
            if user_picked_layout.get() {
                return;
            }
            let layouts_ref = layouts.borrow();
            let Some(code) = layout_for_locale(locale, &layouts_ref) else {
                return;
            };
            let Some(index) = layouts_ref.iter().position(|layout| layout.code == code) else {
                return;
            };
            drop(layouts_ref);

            populating.set(true);
            layout_dropdown.set_selected(index as u32);
            populating.set(false);
            on_layout_changed(index);
        })
    }
}

impl Step for KeyboardStep {
    fn id(&self) -> StepId {
        StepId::Keyboard
    }

    fn title(&self) -> String {
        tr("Keyboard")
    }

    fn icon_name(&self) -> &'static str {
        "keyboard-symbolic"
    }

    fn widget(&self) -> gtk::Widget {
        self.widget.clone()
    }

    fn validity(&self) -> ValidityTracker {
        self.validity.clone()
    }

    fn commit(&self, cfg: &mut InstallConfig) -> mx::Result<()> {
        cfg.keyboard_layout = self.selected_layout.borrow().clone();
        cfg.keyboard_variant = self.selected_variant.borrow().clone();
        cfg.console_keymap = self.selected_layout.borrow().clone();
        Ok(())
    }

    fn retranslate(&self) {
        self.group.set_title(&tr("Keyboard"));
        self.layout_row.set_title(&tr("Layout"));
        self.variant_row.set_title(&tr("Variant"));
        self.test_row.set_title(&tr("Typing test"));
        if self.error_banner.is_revealed() {
            self.error_banner.set_title(&tr(
                "This layout could not be applied to the running session.",
            ));
        }

        // Layout and variant names come from xkeyboard-config's catalogs, so
        // they move with the language too — and their alphabetical order with
        // them. Selections are restored by code, not by index.
        let layout_code = self.selected_layout.borrow().clone();
        let variant_code = self.selected_variant.borrow().clone();
        let mut layouts = self.layouts.borrow_mut();
        if layouts.is_empty() {
            return;
        }
        sort_layouts(&mut layouts);
        fill_layout_dropdown(
            &self.layout_dropdown,
            &layouts,
            &layout_code,
            &self.populating,
        );
        let layout = layouts.iter().find(|l| l.code == layout_code);
        fill_variant_dropdown(
            &self.variant_dropdown,
            layout,
            &variant_code,
            &self.populating,
        );
    }
}
