//! Password + confirmation pair with live strength/mismatch feedback, shared
//! by the user step and step 7's encryption passphrase. Exposes only the raw
//! rows/labels — no `PreferencesGroup`/`PreferencesPage` of its own, since
//! where those rows need to live (a plain group, or an `AdwExpanderRow`)
//! differs per caller; see `attach_to_group`/`attach_to_expander`.

use super::password_strength;
use crate::i18n::tr;
use adw::prelude::*;
use std::cell::Cell;
use std::rc::Rc;

#[derive(Clone)]
pub struct PasswordConfirmEntry {
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

    let show_strength = !password.is_empty() && !password_strength::is_strong_enough(&password);
    let strength_text = if show_strength {
        strength_hint()
    } else {
        String::new()
    };
    strength_label.set_label(&strength_text);
    strength_label.set_visible(show_strength);

    let is_match = password == confirm || confirm.is_empty();
    matches.set(is_match);
    let show_error = !confirm.is_empty() && !is_match;
    if show_error {
        error_label.set_label(&mismatch_error());
        confirm_row.add_css_class("error");
        confirm_row.update_state(&[gtk::accessible::State::Invalid(
            gtk::AccessibleInvalidState::True,
        )]);
    } else {
        error_label.set_label("");
        confirm_row.remove_css_class("error");
        confirm_row.update_state(&[gtk::accessible::State::Invalid(
            gtk::AccessibleInvalidState::False,
        )]);
    }
    error_label.set_visible(show_error);
    strength_label.update_property(&[gtk::accessible::Property::Label(&strength_label.text())]);
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
        confirm_row.update_relation(&[gtk::accessible::Relation::DescribedBy(&[
            error_label.upcast_ref()
        ])]);

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
            password_row,
            confirm_row,
            strength_label,
            error_label,
            matches,
        }
    }

    /// Adds the password/confirm rows and their hint labels to a plain
    /// `AdwPreferencesGroup` — the user step's case.
    pub fn attach_to_group(&self, group: &adw::PreferencesGroup) {
        group.add(&self.password_row);
        group.add(&self.strength_label);
        group.add(&self.confirm_row);
        group.add(&self.error_label);
    }

    /// Adds the same rows as child rows of an `AdwExpanderRow` — step 7's
    /// encryption toggle, so the passphrase fields expand/collapse in place
    /// instead of living behind a separate `GtkRevealer`. Each hint sits
    /// directly under the field it describes (strength under the password,
    /// mismatch under the confirmation) and is hidden entirely — not shown
    /// as an empty row — while there's nothing to say (see `refresh_hints`).
    pub fn attach_to_expander(&self, row: &adw::ExpanderRow) {
        row.add_row(&self.password_row);
        row.add_row(&self.strength_label);
        row.add_row(&self.confirm_row);
        row.add_row(&self.error_label);
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
        .visible(false)
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
