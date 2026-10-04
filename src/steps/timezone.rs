use crate::backend::Backends;
use crate::bridge;
use crate::config::InstallConfig;
use crate::i18n::tr;
use crate::mx;
use crate::steps::{Step, StepId, ValidityTracker};
use crate::widgets::{TimezoneMap, enable_string_search, size_dropdown_to_widest};
use adw::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;

const DEFAULT_TIMEZONE: &str = "Europe/Paris";

/// Step 3 — clickable world map plus a plain dropdown fallback, kept in sync
/// with each other.
pub struct TimezoneStep {
    widget: gtk::Widget,
    group: adw::PreferencesGroup,
    fallback_row: adw::ActionRow,
    selected: Rc<RefCell<String>>,
    validity: ValidityTracker,
}

impl TimezoneStep {
    pub fn new(backends: &Backends, runtime: &tokio::runtime::Handle) -> Self {
        let selected = Rc::new(RefCell::new(DEFAULT_TIMEZONE.to_string()));

        let map = TimezoneMap::new();

        let dropdown = gtk::DropDown::from_strings(&[DEFAULT_TIMEZONE]);
        enable_string_search(&dropdown);
        let fallback_row = adw::ActionRow::builder()
            .title(tr("Timezone"))
            .subtitle(tr("Click the map or pick from the list"))
            .build();
        fallback_row.add_suffix(&dropdown);

        let group = adw::PreferencesGroup::builder()
            .title(tr("Timezone"))
            .build();
        group.add(&fallback_row);

        let container = gtk::Box::new(gtk::Orientation::Vertical, 12);
        container.append(&map.widget());

        let page = adw::PreferencesPage::new();
        page.add(&group);
        container.append(&page);

        let zone_names: Rc<RefCell<Vec<String>>> =
            Rc::new(RefCell::new(vec![DEFAULT_TIMEZONE.to_string()]));

        {
            let locale_backend = backends.locale.clone();
            let map = map.clone();
            let dropdown = dropdown.clone();
            let zone_names = zone_names.clone();
            let selected = selected.clone();
            bridge::spawn(
                runtime,
                async move { locale_backend.list_timezones().await },
                move |result| {
                    let Ok(entries) = result else { return };
                    if entries.is_empty() {
                        return;
                    }
                    let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
                    let model = gtk::StringList::new(&names);
                    size_dropdown_to_widest(&dropdown, &model);
                    dropdown.set_model(Some(&model));
                    *zone_names.borrow_mut() = entries.iter().map(|e| e.name.clone()).collect();
                    let default_idx = entries.iter().position(|e| e.name == DEFAULT_TIMEZONE);
                    *selected.borrow_mut() = default_idx
                        .map(|i| entries[i].name.clone())
                        .unwrap_or_else(|| entries[0].name.clone());
                    if let Some(idx) = default_idx {
                        dropdown.set_selected(idx as u32);
                    }
                    map.set_entries(entries);
                    map.select_by_name(&selected.borrow());
                },
            );
        }

        {
            let selected = selected.clone();
            let dropdown_for_sync = dropdown.clone();
            let zone_names = zone_names.clone();
            map.connect_selected(move |entry| {
                *selected.borrow_mut() = entry.name.clone();
                if let Some(idx) = zone_names.borrow().iter().position(|n| n == &entry.name) {
                    dropdown_for_sync.set_selected(idx as u32);
                }
            });
        }

        {
            let selected = selected.clone();
            let zone_names = zone_names.clone();
            let map = map.clone();
            dropdown.connect_selected_notify(move |dd| {
                let idx = dd.selected() as usize;
                if let Some(name) = zone_names.borrow().get(idx) {
                    *selected.borrow_mut() = name.clone();
                    map.select_by_name(name);
                }
            });
        }

        Self {
            widget: container.upcast(),
            group,
            fallback_row,
            selected,
            validity: ValidityTracker::ready(),
        }
    }
}

impl Step for TimezoneStep {
    fn id(&self) -> StepId {
        StepId::Timezone
    }

    fn title(&self) -> String {
        tr("Timezone")
    }

    fn icon_name(&self) -> &'static str {
        "timezone-symbolic"
    }

    fn widget(&self) -> gtk::Widget {
        self.widget.clone()
    }

    fn validity(&self) -> ValidityTracker {
        self.validity.clone()
    }

    fn commit(&self, cfg: &mut InstallConfig) -> mx::Result<()> {
        cfg.timezone = self.selected.borrow().clone();
        Ok(())
    }

    fn retranslate(&self) {
        self.group.set_title(&tr("Timezone"));
        self.fallback_row.set_title(&tr("Timezone"));
        self.fallback_row
            .set_subtitle(&tr("Click the map or pick from the list"));
    }
}
