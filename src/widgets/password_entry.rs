//! Password + confirmation pair with live strength/mismatch feedback (user
//! step). Wrapped in its own `PreferencesPage` so it gets the same clamped
//! reading width as every other step — a bare `PreferencesGroup` dropped
//! straight into a `gtk::Box` has no width clamp and stretches full-window.

use super::password_strength;
use crate::i18n::tr;
use adw::prelude::*;
use std::cell::Cell;
use std::rc::Rc;

#[derive(Clone)]
pub struct PasswordConfirmEntry {
    widget: gtk::Widget,
    password_row: adw::PasswordEntryRow,
    confirm_row: adw::PasswordEntryRow,
    strength_label: gtk::Label,
    error_label: gtk::Label,
    matches: Rc<Cell<bool>>,
}

fn strength_hint() -> String {
    tr(
        "For a strong password, use one of (CNIL recommendation):\n - 12+ characters with uppercase, lowercase, digits and special characters\n - 14+ characters with uppercase, lowercase and digits\n - Passphrase of at least 7 words.\nOptional, but recommended.",
    )
}

fn mismatch_error() -> String {
    tr("Passwords do not match.")
}

/// Recomputes the strength hint and mismatch error from the rows' current
/// text — shared between the live `connect_changed` handlers and
/// `retranslate()` (called after a language change, when the *text* of the
/// hints themselves needs updating even though the passwords didn't).
fn refresh_hints(
    password_row: &adw::PasswordEntryRow,
    confirm_row: &adw::PasswordEntryRow,
    strength_label: &gtk::Label,
    error_label: &gtk::Label,
    matches: &Cell<bool>,
) {
    let password = password_row.text();
    let confirm = confirm_row.text();

    if password.is_empty() || password_strength::is_strong_enough(&password) {
        strength_label.set_label("");
    } else {
        strength_label.set_label(&strength_hint());
    }

    let is_match = password == confirm || confirm.is_empty();
    matches.set(is_match);
    if !confirm.is_empty() && !is_match {
        error_label.set_label(&mismatch_error());
        confirm_row.add_css_class("error");
    } else {
        error_label.set_label("");
        confirm_row.remove_css_class("error");
    }
}

impl PasswordConfirmEntry {
    pub fn new() -> Self {
        let password_row = adw::PasswordEntryRow::builder()
            .title(tr("Password"))
            .build();
        let confirm_row = adw::PasswordEntryRow::builder()
            .title(tr("Confirm password"))
            .build();

        let strength_label = hint_label(&["warning", "caption"]);
        let error_label = hint_label(&["error", "caption"]);

        let group = adw::PreferencesGroup::new();
        group.add(&password_row);
        group.add(&confirm_row);
        group.add(&strength_label);
        group.add(&error_label);

        let page = adw::PreferencesPage::new();
        page.add(&group);

        let matches = Rc::new(Cell::new(true));

        let update = {
            let password_row = password_row.clone();
            let confirm_row = confirm_row.clone();
            let strength_label = strength_label.clone();
            let error_label = error_label.clone();
            let matches = matches.clone();
            move || {
                refresh_hints(
                    &password_row,
                    &confirm_row,
                    &strength_label,
                    &error_label,
                    &matches,
                )
            }
        };

        {
            let update = update.clone();
            password_row.connect_changed(move |_| update());
        }
        confirm_row.connect_changed(move |_| update());

        Self {
            widget: page.upcast(),
            password_row,
            confirm_row,
            strength_label,
            error_label,
            matches,
        }
    }

    pub fn widget(&self) -> gtk::Widget {
        self.widget.clone()
    }

    pub fn password(&self) -> String {
        self.password_row.text().to_string()
    }

    /// True only once both fields are non-empty and equal. Strength is
    /// advisory (CNIL recommendation) and never blocks the wizard.
    pub fn is_valid(&self) -> bool {
        !self.password_row.text().is_empty()
            && self.confirm_row.text() == self.password_row.text()
            && self.matches.get()
    }

    pub fn connect_changed<F: Fn() + 'static>(&self, f: F) {
        let f = Rc::new(f);
        {
            let f = f.clone();
            self.password_row.connect_changed(move |_| f());
        }
        self.confirm_row.connect_changed(move |_| f());
    }

    pub fn retranslate(&self) {
        self.password_row.set_title(&tr("Password"));
        self.confirm_row.set_title(&tr("Confirm password"));
        refresh_hints(
            &self.password_row,
            &self.confirm_row,
            &self.strength_label,
            &self.error_label,
            &self.matches,
        );
    }
}

fn hint_label(css_classes: &[&str]) -> gtk::Label {
    let label = gtk::Label::builder()
        .wrap(true)
        .xalign(0.0)
        .margin_top(2)
        .margin_bottom(2)
        .margin_start(12)
        .margin_end(12)
        .build();
    for class in css_classes {
        label.add_css_class(class);
    }
    label
}

impl Default for PasswordConfirmEntry {
    fn default() -> Self {
        Self::new()
    }
}
