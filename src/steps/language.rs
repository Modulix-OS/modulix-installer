use crate::backend::Backends;
use crate::backend::locale::{display_name, language_code_of};
use crate::bridge;
use crate::config::InstallConfig;
use crate::i18n::{self, tr};
use crate::mx;
use crate::steps::{LanguageHook, RetranslateHook, Step, StepId, ValidityTracker};
use crate::widgets::{enable_string_search, size_dropdown_to_widest};
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
        language_hook: LanguageHook,
    ) -> Self {
        let codes: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
        // `i18n::init()` already seeded the startup message language —
        // `i18n::DEFAULT_LANGUAGE` (English), never the host locale — before
        // any widget was built, so this starts out matching reality and the
        // dropdown lands on English once the model is populated below.
        let selected = Rc::new(RefCell::new(i18n::current_language_code()));
        // Set while `set_model`/`set_selected` run below: replacing the model
        // fires `notify::selected` synchronously, and without this guard that
        // spurious event would immediately overwrite the already-correct
        // startup language with whatever ends up at the pre-population index.
        let populating = Rc::new(Cell::new(true));

        let dropdown = gtk::DropDown::from_strings(&[]);
        enable_string_search(&dropdown);

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
                    let Ok(locales) = result else {
                        populating.set(false);
                        return;
                    };
                    if locales.is_empty() {
                        populating.set(false);
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
                    let model = gtk::StringList::new(&names);
                    size_dropdown_to_widest(&dropdown, &model);
                    dropdown.set_model(Some(&model));

                    // Reflect the already-active startup language in the
                    // dropdown instead of leaving it on whatever the model
                    // defaulted to. Compared normalized (charset stripped,
                    // case-folded): codes here come from `list_locales()`
                    // (SUPPORTED-derived, e.g. `fr_FR.UTF-8`) while `current`
                    // may come from `locale -a` (e.g. `fr_FR.utf8`) on a
                    // system without `/usr/share/i18n/SUPPORTED` — a naive
                    // string compare misses that match.
                    let current = selected.borrow().clone();
                    if let Some(idx) = entries
                        .iter()
                        .position(|(_, code)| i18n::normalized_eq(code, &current))
                    {
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
                // The UI language always switches (`LANGUAGE`-driven, see
                // i18n.rs); `MessagesOnly` only means `setlocale` couldn't
                // land on `code` for date/number formatting because it isn't
                // generated on this machine — not worth a toast, but worth
                // tracing.
                if i18n::set_language(code) == i18n::LanguageOutcome::MessagesOnly {
                    eprintln!(
                        "i18n: switched messages to {code:?} but setlocale() couldn't apply it \
                         (not generated on this machine) — formatting stays on the previous locale"
                    );
                }
                (retranslate_hook.borrow())();
                // After the retranslate pass, so a step that rebuilds a list
                // from this hook does it with names already in the new
                // language.
                (language_hook.borrow())(code);
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
