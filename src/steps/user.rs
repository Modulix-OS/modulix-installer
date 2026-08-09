use crate::config::InstallConfig;
use crate::i18n::tr;
use crate::mx;
use crate::steps::{Step, StepId, ValidityTracker};
use crate::widgets::PasswordConfirmEntry;
use adw::prelude::*;

/// Step 8 — primary user; root receives the same password (see CLAUDE.md:
/// both must end up `hashedPassword`, not `initialPassword`, once `init_all`
/// lands in iteration 2).
pub struct UserStep {
    widget: gtk::Widget,
    group: adw::PreferencesGroup,
    username_row: adw::EntryRow,
    fullname_row: adw::EntryRow,
    password_widget: PasswordConfirmEntry,
    validity: ValidityTracker,
}

impl UserStep {
    pub fn new() -> Self {
        let username_row = adw::EntryRow::builder().title(tr("Username")).build();
        let fullname_row = adw::EntryRow::builder().title(tr("Full name")).build();
        let password_widget = PasswordConfirmEntry::new();

        let group = adw::PreferencesGroup::builder()
            .title(tr("User account"))
            .build();
        group.add(&username_row);
        group.add(&fullname_row);

        let page = adw::PreferencesPage::new();
        page.add(&group);

        let container = gtk::Box::new(gtk::Orientation::Vertical, 12);
        container.append(&page);
        container.append(&password_widget.widget());

        let validity = ValidityTracker::blocked(tr("Enter a username"));

        let update_validity = {
            let username_row = username_row.clone();
            let password_widget = password_widget.clone();
            let validity = validity.clone();
            move || {
                if username_row.text().trim().is_empty() {
                    validity.set_blocked(tr("Enter a username"));
                } else if !password_widget.is_valid() {
                    validity.set_blocked(tr("Passwords must match and not be empty"));
                } else {
                    validity.set_ready();
                }
            }
        };

        {
            let update_validity = update_validity.clone();
            username_row.connect_changed(move |_| update_validity());
        }
        password_widget.connect_changed(update_validity);

        Self {
            widget: container.upcast(),
            group,
            username_row,
            fullname_row,
            password_widget,
            validity,
        }
    }
}

impl Default for UserStep {
    fn default() -> Self {
        Self::new()
    }
}

impl Step for UserStep {
    fn id(&self) -> StepId {
        StepId::User
    }

    fn title(&self) -> String {
        tr("User")
    }

    fn icon_name(&self) -> &'static str {
        "user-symbolic"
    }

    fn widget(&self) -> gtk::Widget {
        self.widget.clone()
    }

    fn validity(&self) -> ValidityTracker {
        self.validity.clone()
    }

    fn commit(&self, cfg: &mut InstallConfig) -> mx::Result<()> {
        cfg.user.username = self.username_row.text().trim().to_string();
        cfg.user.full_name = self.fullname_row.text().to_string();
        cfg.user.password = self.password_widget.password();
        Ok(())
    }

    fn retranslate(&self) {
        self.group.set_title(&tr("User account"));
        self.username_row.set_title(&tr("Username"));
        self.fullname_row.set_title(&tr("Full name"));
        self.password_widget.retranslate();
    }
}
