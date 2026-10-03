use crate::backend::Backends;
use crate::backend::locale::KeyboardLayout;
use crate::bridge;
use crate::config::InstallConfig;
use crate::i18n::tr;
use crate::mx;
use crate::steps::{Step, StepId, ValidityTracker};
use crate::widgets::size_dropdown_to_widest;
use adw::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;

const NO_VARIANT: &str = "";

pub struct KeyboardStep {
    widget: gtk::Widget,
    group: adw::PreferencesGroup,
    layout_row: adw::ActionRow,
    variant_row: adw::ActionRow,
    test_row: adw::ActionRow,
    error_banner: adw::Banner,
    selected_layout: Rc<RefCell<String>>,
    selected_variant: Rc<RefCell<String>>,
    validity: ValidityTracker,
}

impl KeyboardStep {
    pub fn new(backends: &Backends, runtime: &tokio::runtime::Handle) -> Self {
        let layouts: Rc<RefCell<Vec<KeyboardLayout>>> = Rc::new(RefCell::new(Vec::new()));
        let selected_layout = Rc::new(RefCell::new("us".to_string()));
        let selected_variant = Rc::new(RefCell::new(NO_VARIANT.to_string()));

        let layout_dropdown = gtk::DropDown::from_strings(&["English (US)"]);
        let layout_row = adw::ActionRow::builder().title(tr("Layout")).build();
        layout_row.add_suffix(&layout_dropdown);

        let variant_dropdown = gtk::DropDown::from_strings(&["Default"]);
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

        let apply_layout = {
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
        };

        {
            let locale_backend = backends.locale.clone();
            let layouts = layouts.clone();
            let layout_dropdown = layout_dropdown.clone();
            let selected_layout = selected_layout.clone();
            let apply_layout = apply_layout.clone();
            bridge::spawn(
                runtime,
                async move { locale_backend.list_keyboard_layouts().await },
                move |result| {
                    let Ok(loaded) = result else { return };
                    if loaded.is_empty() {
                        return;
                    }
                    let names: Vec<&str> = loaded.iter().map(|l| l.description.as_str()).collect();
                    let model = gtk::StringList::new(&names);
                    size_dropdown_to_widest(&layout_dropdown, &model);
                    layout_dropdown.set_model(Some(&model));
                    *selected_layout.borrow_mut() = loaded[0].code.clone();
                    apply_layout(loaded[0].code.clone(), NO_VARIANT.to_string());
                    *layouts.borrow_mut() = loaded;
                },
            );
        }

        {
            let layouts = layouts.clone();
            let selected_layout = selected_layout.clone();
            let selected_variant = selected_variant.clone();
            let variant_dropdown = variant_dropdown.clone();
            let apply_layout = apply_layout.clone();
            layout_dropdown.connect_selected_notify(move |dd| {
                let idx = dd.selected() as usize;
                let layouts_ref = layouts.borrow();
                let Some(layout) = layouts_ref.get(idx) else {
                    return;
                };
                *selected_layout.borrow_mut() = layout.code.clone();
                *selected_variant.borrow_mut() = NO_VARIANT.to_string();

                let mut variant_names: Vec<&str> = vec!["Default"];
                variant_names.extend(layout.variants.iter().map(|v| v.description.as_str()));
                let variant_model = gtk::StringList::new(&variant_names);
                size_dropdown_to_widest(&variant_dropdown, &variant_model);
                variant_dropdown.set_model(Some(&variant_model));
                variant_dropdown.set_selected(0);

                apply_layout(layout.code.clone(), NO_VARIANT.to_string());
            });
        }

        {
            let layouts = layouts.clone();
            let selected_layout = selected_layout.clone();
            let selected_variant = selected_variant.clone();
            layout_dropdown_variant_notify(&variant_dropdown, move |idx| {
                let layouts_ref = layouts.borrow();
                let Some(layout) = layouts_ref
                    .iter()
                    .find(|l| l.code == *selected_layout.borrow())
                else {
                    return;
                };
                let variant_code = if idx == 0 {
                    NO_VARIANT.to_string()
                } else {
                    layout
                        .variants
                        .get(idx - 1)
                        .map(|v| v.code.clone())
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
            selected_layout,
            selected_variant,
            validity: ValidityTracker::ready(),
        }
    }
}

fn layout_dropdown_variant_notify<F: Fn(usize) + 'static>(dropdown: &gtk::DropDown, f: F) {
    dropdown.connect_selected_notify(move |dd| f(dd.selected() as usize));
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
    }
}
