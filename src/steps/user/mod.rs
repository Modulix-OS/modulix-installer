use crate::config::InstallConfig;
use crate::i18n::tr;
use crate::mx;
use crate::steps::{Step, StepId, ValidityTracker};
use crate::widgets::PasswordConfirmEntry;
use adw::prelude::*;
use std::cell::Cell;
use std::rc::Rc;
mod hostname;
mod username;

/// Primary user and computer name; root receives the same password.
/// Neither password is written into the NixOS configuration: they are set
/// after the install with `chpasswd` under `nixos-enter`
/// (`engine::tasks::SetPasswordsTask`), so nothing plaintext reaches the
/// Nix store.
pub struct UserStep {
    widget: gtk::Widget,
    group: adw::PreferencesGroup,
    username_row: adw::EntryRow,
    fullname_row: adw::EntryRow,
    hostname_row: adw::EntryRow,
    password_widget: PasswordConfirmEntry,
    validity: ValidityTracker,
}

impl UserStep {
    pub fn new() -> Self {
        let fullname_row = adw::EntryRow::builder().title(tr("Full name")).build();
        let username_row = adw::EntryRow::builder().title(tr("Username")).build();
        username_row.set_tooltip_text(Some(&tr("Lowercase letters, digits, - and _ only")));
        let hostname_row = adw::EntryRow::builder().title(tr("Computer name")).build();
        hostname_row.set_text(hostname::DEFAULT);
        let password_widget = PasswordConfirmEntry::new();

        let group = adw::PreferencesGroup::builder()
            .title(tr("User account"))
            .build();
        group.add(&fullname_row);
        group.add(&username_row);
        group.add(&hostname_row);
        password_widget.attach_to_group(&group);

        let page = adw::PreferencesPage::new();
        page.add(&group);

        let container = gtk::Box::new(gtk::Orientation::Vertical, 12);
        container.append(&page);

        let validity = ValidityTracker::blocked(tr("Enter a username"));

        let update_validity = {
            let username_row = username_row.clone();
            let hostname_row = hostname_row.clone();
            let password_widget = password_widget.clone();
            let validity = validity.clone();
            move || {
                if let Some(reason) = username::blocked_reason(&username_row.text()) {
                    validity.set_blocked(reason);
                } else if let Some(reason) = hostname::blocked_reason(&hostname_row.text()) {
                    validity.set_blocked(reason);
                } else if !password_widget.is_valid() {
                    validity.set_blocked(tr("Passwords must match and not be empty"));
                } else {
                    validity.set_ready();
                }
            }
        };

        let syncing = Rc::new(Cell::new(false));
        let user_edited = Rc::new(Cell::new(false));

        {
            let username_row = username_row.clone();
            let syncing = syncing.clone();
            let user_edited = user_edited.clone();
            let update_validity = update_validity.clone();
            fullname_row.connect_changed(move |row| {
                if user_edited.get() {
                    return;
                }
                let derived = username::from_full_name(&row.text());
                syncing.set(true);
                username_row.set_text(&derived);
                syncing.set(false);
                update_validity();
            });
        }

        {
            let syncing = syncing.clone();
            let user_edited = user_edited.clone();
            let update_validity = update_validity.clone();
            username_row.connect_changed(move |row| {
                if syncing.get() {
                    return;
                }
                let text = row.text();
                user_edited.set(!text.is_empty());

                let clean = username::sanitize(&text);
                if clean != text {
                    let old_pos = row.position().max(0) as usize;
                    let kept_before_cursor = text
                        .chars()
                        .take(old_pos)
                        .filter(|c| {
                            c.is_ascii_lowercase()
                                || c.is_ascii_digit()
                                || *c == '_'
                                || *c == '-'
                                || c.is_ascii_uppercase()
                        })
                        .count();
                    syncing.set(true);
                    row.set_text(&clean);
                    syncing.set(false);
                    row.set_position(kept_before_cursor.min(clean.chars().count()) as i32);
                }
                update_validity();
            });
        }

        {
            let syncing = syncing.clone();
            let update_validity = update_validity.clone();
            hostname_row.connect_changed(move |row| {
                if syncing.get() {
                    return;
                }
                let text = row.text();
                let clean = hostname::sanitize(&text);
                if clean != text {
                    let pos = row.position().max(0) as usize;
                    syncing.set(true);
                    row.set_text(&clean);
                    syncing.set(false);
                    row.set_position(pos.min(clean.chars().count()) as i32);
                }
                update_validity();
            });
        }

        password_widget.connect_changed(update_validity);

        Self {
            widget: container.upcast(),
            group,
            username_row,
            fullname_row,
            hostname_row,
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
        cfg.user.username = username::sanitize(&self.username_row.text());
        cfg.user.full_name = self.fullname_row.text().trim().to_string();
        cfg.user.hostname = hostname::sanitize(&self.hostname_row.text());
        cfg.user.password = self.password_widget.password();
        Ok(())
    }

    fn retranslate(&self) {
        self.group.set_title(&tr("User account"));
        self.username_row.set_title(&tr("Username"));
        self.username_row
            .set_tooltip_text(Some(&tr("Lowercase letters, digits, - and _ only")));
        self.fullname_row.set_title(&tr("Full name"));
        self.hostname_row.set_title(&tr("Computer name"));
        self.password_widget.retranslate();
    }
}
