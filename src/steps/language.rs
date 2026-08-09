use crate::backend::Backends;
use crate::backend::locale::{display_name, language_code_of};
use crate::bridge;
use crate::config::InstallConfig;
use crate::i18n::{self, tr};
use crate::mx;
use crate::steps::{RetranslateHook, Step, StepId, ValidityTracker};
use adw::prelude::*;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

/// Step 2 — changing the selection retranslates every already-built step
/// (via `retranslate_hook`) without rebuilding any of them.
pub struct LanguageStep {
    widget: gtk::Widget,
    group: adw::PreferencesGroup,
    row: adw::ActionRow,
    selected: Rc<RefCell<String>>,
    validity: ValidityTracker,
}

impl LanguageStep {
    pub fn new(
        backends: &Backends,
        runtime: &tokio::runtime::Handle,
        retranslate_hook: RetranslateHook,
    ) -> Self {
        let codes: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
        // `i18n::init()` already applied the process locale (from $LANG) before
        // any widget was built, so this starts out matching reality — updated
        // below once we know the real code, not left at a fake placeholder.
        let selected = Rc::new(RefCell::new(std::env::var("LANG").unwrap_or_default()));
        // Set while `set_model`/`set_selected` run below: replacing the model
        // fires `notify::selected` synchronously, and without this guard that
        // spurious event would immediately overwrite the already-correct
        // startup language with whatever ends up at the pre-population index.
        let populating = Rc::new(Cell::new(true));

        let dropdown = gtk::DropDown::from_strings(&[]);

        let row = adw::ActionRow::builder()
            .title(tr("Language"))
            .subtitle(tr("Used for the installer and the installed system"))
            .build();
        row.add_suffix(&dropdown);

        let group = adw::PreferencesGroup::builder()
            .title(tr("Language"))
            .build();
        group.add(&row);

        let page = adw::PreferencesPage::new();
        page.add(&group);

        {
            let locale_backend = backends.locale.clone();
            let dropdown = dropdown.clone();
            let codes = codes.clone();
            let selected = selected.clone();
            let populating = populating.clone();
            bridge::spawn(
                runtime,
                async move { locale_backend.list_locales().await },
                move |result| {
                    let Ok(locales) = result else { return };
                    if locales.is_empty() {
                        return;
                    }

                    let mut territory_counts: HashMap<String, usize> = HashMap::new();
                    for locale in &locales {
                        *territory_counts
                            .entry(language_code_of(&locale.code).to_string())
                            .or_insert(0) += 1;
                    }

                    let mut entries: Vec<(String, String)> = locales
                        .into_iter()
                        .map(|locale| {
                            let show_territory = territory_counts
                                .get(language_code_of(&locale.code))
                                .copied()
                                .unwrap_or(0)
                                > 1;
                            (display_name(&locale.code, show_territory), locale.code)
                        })
                        .collect();
                    entries.sort_by(|a, b| a.0.cmp(&b.0));

                    let names: Vec<&str> = entries.iter().map(|(name, _)| name.as_str()).collect();
                    dropdown.set_model(Some(&gtk::StringList::new(&names)));

                    // Reflect the already-active startup language in the
                    // dropdown instead of leaving it on whatever the model
                    // defaulted to.
                    let current = selected.borrow().clone();
                    if let Some(idx) = entries.iter().position(|(_, code)| *code == current) {
                        dropdown.set_selected(idx as u32);
                    }

                    *codes.borrow_mut() = entries.into_iter().map(|(_, code)| code).collect();
                    populating.set(false);
                },
            );
        }

        let selected_state = selected.clone();
        let codes_for_signal = codes.clone();
        dropdown.connect_selected_notify(move |dd| {
            if populating.get() {
                return;
            }
            let idx = dd.selected() as usize;
            if let Some(code) = codes_for_signal.borrow().get(idx) {
                *selected_state.borrow_mut() = code.clone();
                i18n::set_language(code);
                (retranslate_hook.borrow())();
            }
        });

        Self {
            widget: page.upcast(),
            group,
            row,
            selected,
            validity: ValidityTracker::ready(),
        }
    }
}

impl Step for LanguageStep {
    fn id(&self) -> StepId {
        StepId::Language
    }

    fn title(&self) -> String {
        tr("Language")
    }

    fn icon_name(&self) -> &'static str {
        "language-symbolic"
    }

    fn widget(&self) -> gtk::Widget {
        self.widget.clone()
    }

    fn validity(&self) -> ValidityTracker {
        self.validity.clone()
    }

    fn commit(&self, cfg: &mut InstallConfig) -> mx::Result<()> {
        cfg.language = self.selected.borrow().clone();
        Ok(())
    }

    fn retranslate(&self) {
        self.group.set_title(&tr("Language"));
        self.row.set_title(&tr("Language"));
        self.row
            .set_subtitle(&tr("Used for the installer and the installed system"));
    }
}
