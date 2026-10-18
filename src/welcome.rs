//! Pre-wizard welcome page: one oversized greeting that cross-fades to the
//! next language every [`ROTATE_SECONDS`] seconds, over a single blue button
//! that starts the wizard.
//!
//! The strings here are **not** gettext msgids and must not become any: the
//! page shows every language in turn regardless of the active locale (the
//! language step hasn't run yet), so each greeting has to exist in all of them
//! at once. Adding a language is one [`Greeting`] entry.
//!
//! Each entry does carry the locale it is written in, because clicking the
//! button adopts the language of the greeting on screen — the caller passes
//! that code to the language step (see `crate::steps::SelectLanguageHook`), so
//! the wizard, the language dropdown and `InstallConfig::language` all start on
//! the language the user actually read.
//!
//! The rotation is unconditional: it is deliberately *not* paused while the
//! button is hovered or focused. The button is the only focusable widget on the
//! page, so GTK focuses it on the first frame and a focus-driven pause froze
//! the rotation for the whole session.
//!
//! Nothing of the wizard is visible from this page — no step rail, no header
//! buttons; [`crate::app::build_window`] keeps it in a `gtk::Stack` ahead of
//! the `NavigationSplitView` and swaps to the wizard when the button fires.

use adw::prelude::*;
use std::cell::Cell;
use std::rc::Rc;

/// Seconds a greeting stays on screen before the next language fades in.
const ROTATE_SECONDS: u32 = 3;

/// Cross-fade duration, in milliseconds. Well under [`ROTATE_SECONDS`] so the
/// text is readable at rest rather than permanently in transition.
const FADE_MS: u32 = 700;

/// CSS the welcome page needs, appended to the window-wide provider by
/// [`crate::app::build_window`] (one provider for the whole app).
pub const CSS: &str = "
.mx-welcome-title {
  font-size: 56px;
  font-weight: 800;
}
.mx-welcome-start {
  font-size: 18px;
  padding: 14px 36px;
}
";

/// One language's rendering of the whole page.
///
/// * `title` - the large greeting.
/// * `button` - label of the start button, in the same language as `title`.
/// * `locale` - full locale code the two strings are written in, handed to
///   `on_start` when this greeting is the visible one. Full (territory-carrying)
///   for the same reason [`crate::i18n::DEFAULT_LANGUAGE`] is: it is matched
///   against `LocaleBackend::list_locales()` entries, which all carry one.
struct Greeting {
    title: &'static str,
    button: &'static str,
    locale: &'static str,
}

/// Greetings cycled through, in display order. One entry per language whose
/// catalog exists today (`po/` ships `fr.po`, `es.po` and `de.po`, plus the
/// English `msgid` fallback); extend as translations land.
///
/// English comes first on purpose: the startup language is
/// [`crate::i18n::DEFAULT_LANGUAGE`], so the first frame shows the language the
/// installer is actually running in.
const GREETINGS: &[Greeting] = &[
    Greeting {
        title: "Welcome on Modulix OS",
        button: "Start the installation",
        locale: "en_US.UTF-8",
    },
    Greeting {
        title: "Bienvenue sur Modulix OS",
        button: "Commencer l'installation",
        locale: "fr_FR.UTF-8",
    },
    Greeting {
        title: "Bienvenido a Modulix OS",
        button: "Comenzar la instalación",
        locale: "es_ES.UTF-8",
    },
    Greeting {
        title: "Willkommen bei Modulix OS",
        button: "Installation starten",
        locale: "de_DE.UTF-8",
    },
];

/// Builds the welcome page.
///
/// * `on_start` - run when the start button is clicked, with the `locale` of the
///   greeting then on screen; expected to apply that language and reveal the
///   wizard. Called once per click, on the GTK main thread.
///
/// # Pre-conditions
/// [`CSS`] is loaded into a style provider for the display, or the greeting
/// renders at body size.
///
/// # Post-conditions
/// A rotation timer is armed and advances both stacks together; it stops on its
/// next wake-up once the returned widget has been dropped, so the page can be
/// discarded without any explicit teardown. The locale `on_start` receives is
/// always the one the visible greeting is written in.
///
/// Returns the page widget, to be added to a container by the caller.
pub fn build_page<F: Fn(&str) + 'static>(on_start: F) -> gtk::Widget {
    let title_stack = gtk::Stack::builder()
        .transition_type(gtk::StackTransitionType::Crossfade)
        .transition_duration(FADE_MS)
        .halign(gtk::Align::Center)
        .build();
    let button_stack = gtk::Stack::builder()
        .transition_type(gtk::StackTransitionType::Crossfade)
        .transition_duration(FADE_MS)
        .halign(gtk::Align::Center)
        .build();

    for (i, greeting) in GREETINGS.iter().enumerate() {
        let name = i.to_string();
        let title = gtk::Label::builder()
            .label(greeting.title)
            .justify(gtk::Justification::Center)
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .css_classes(["mx-welcome-title"])
            .build();
        title_stack.add_named(&title, Some(&name));
        button_stack.add_named(&gtk::Label::new(Some(greeting.button)), Some(&name));
    }

    let index = Rc::new(Cell::new(0usize));

    let button = gtk::Button::builder()
        .child(&button_stack)
        .halign(gtk::Align::Center)
        .css_classes(["suggested-action", "pill", "mx-welcome-start"])
        .build();
    {
        let index = index.clone();
        button.connect_clicked(move |_| on_start(GREETINGS[index.get()].locale));
    }

    let body = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(48)
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .margin_start(48)
        .margin_end(48)
        .build();
    body.append(&title_stack);
    body.append(&button);

    arm_rotation(&title_stack, &button_stack, index);

    body.upcast()
}

/// Arms the repeating timer that cross-fades both stacks to the next language.
///
/// * `title_stack` - greeting stack; held weakly.
/// * `button_stack` - start-button label stack; held weakly.
/// * `index` - shared cursor into [`GREETINGS`], also read by the start button
///   to know which language it is starting in.
///
/// # Post-conditions
/// The timer keeps firing until either stack is gone, then breaks. Both stacks
/// are always advanced to the same index, and `index` always holds it, so title,
/// button and the language handed to `on_start` never disagree.
fn arm_rotation(title_stack: &gtk::Stack, button_stack: &gtk::Stack, index: Rc<Cell<usize>>) {
    let title_weak = title_stack.downgrade();
    let button_weak = button_stack.downgrade();

    glib::timeout_add_seconds_local(ROTATE_SECONDS, move || {
        let (Some(title_stack), Some(button_stack)) = (title_weak.upgrade(), button_weak.upgrade())
        else {
            return glib::ControlFlow::Break;
        };
        let next = (index.get() + 1) % GREETINGS.len();
        index.set(next);
        let name = next.to_string();
        title_stack.set_visible_child_name(&name);
        button_stack.set_visible_child_name(&name);
        glib::ControlFlow::Continue
    });
}

#[cfg(test)]
mod tests {
    use super::GREETINGS;
    use crate::i18n;

    #[test]
    fn first_greeting_is_the_startup_language() {
        assert!(i18n::normalized_eq(
            GREETINGS[0].locale,
            i18n::DEFAULT_LANGUAGE
        ));
    }

    #[test]
    fn greeting_locales_are_distinct() {
        for (i, greeting) in GREETINGS.iter().enumerate() {
            for other in &GREETINGS[i + 1..] {
                assert!(
                    !i18n::normalized_eq(greeting.locale, other.locale),
                    "duplicate greeting locale {:?}",
                    greeting.locale
                );
            }
        }
    }
}
