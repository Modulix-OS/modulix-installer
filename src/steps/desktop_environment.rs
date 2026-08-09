use crate::config::{DesktopEnvironment, InstallConfig};
use crate::i18n::tr;
use crate::mx;
use crate::steps::{Step, StepId, ValidityTracker};
use adw::prelude::*;
use std::cell::Cell;
use std::path::Path;
use std::rc::Rc;

/// Directory `data/screenshots/{gnome,plasma,lxqt}.webp` live in — real
/// screenshot assets aren't part of iteration 1's scaffolding, so this is
/// best-effort: a missing file just leaves the preview blank.
const SCREENSHOT_DIR: &str = "data/screenshots";

fn display_label(de: DesktopEnvironment) -> &'static str {
    match de {
        DesktopEnvironment::Gnome => "GNOME",
        DesktopEnvironment::Plasma => "KDE Plasma",
        DesktopEnvironment::Lxqt => "LXQt",
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
    group: adw::PreferencesGroup,
    rows: Vec<(DesktopEnvironment, adw::ActionRow)>,
    selected: Rc<Cell<DesktopEnvironment>>,
    validity: ValidityTracker,
}

impl DesktopEnvironmentStep {
    pub fn new() -> Self {
        let selected = Rc::new(Cell::new(DesktopEnvironment::default()));

        let preview = gtk::Picture::new();
        preview.set_content_fit(gtk::ContentFit::Contain);
        preview.set_height_request(200);
        set_preview(&preview, DesktopEnvironment::default());

        let group = adw::PreferencesGroup::builder()
            .title(tr("Desktop environment"))
            .build();

        let mut rows = Vec::new();
        for de in DesktopEnvironment::ALL {
            let row = adw::ActionRow::builder()
                .title(tr(display_label(de)))
                .subtitle(tr(description_for(de)))
                .activatable(true)
                .build();

            {
                let selected = selected.clone();
                let preview = preview.clone();
                row.connect_activated(move |_row| {
                    selected.set(de);
                    set_preview(&preview, de);
                });
            }

            group.add(&row);
            rows.push((de, row));
        }

        let page = adw::PreferencesPage::new();
        page.add(&group);

        let container = gtk::Box::new(gtk::Orientation::Vertical, 12);
        container.append(&page);
        container.append(&preview);

        Self {
            widget: container.upcast(),
            group,
            rows,
            selected,
            validity: ValidityTracker::ready(),
        }
    }
}

fn set_preview(preview: &gtk::Picture, de: DesktopEnvironment) {
    let path = Path::new(SCREENSHOT_DIR).join(de.screenshot_file());
    if path.exists() {
        preview.set_filename(Some(&path));
    } else {
        preview.set_filename(None::<&Path>);
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
        self.group.set_title(&tr("Desktop environment"));
        for (de, row) in &self.rows {
            row.set_title(&tr(display_label(*de)));
            row.set_subtitle(&tr(description_for(*de)));
        }
    }
}
