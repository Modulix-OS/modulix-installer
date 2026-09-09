//! Fullscreen wizard window: `NavigationSplitView` (step rail | content),
//! content = `ToolbarView` (header + prev/next) wrapping a `NavigationView`
//! that push/pops between the 9 step pages already built by
//! [`crate::steps::build_registry`].

use crate::a11y::A11ySettings;
use crate::backend::Backends;
use crate::config::InstallConfig;
use crate::finish::progress::ProgressPage;
use crate::finish::summary::SummaryPage;
use crate::i18n::tr;
use crate::steps::{self, Step};
use adw::prelude::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

const RAIL_CSS: &str = "
.step-rail-row { padding: 6px 10px; border-radius: 6px; }
.step-rail-row.current-step { font-weight: bold; background-color: alpha(currentColor, 0.08); }
.mx-warning-strip {
  background-color: var(--warning-bg-color);
  color: var(--warning-fg-color);
  border-radius: 12px;
  padding: 12px;
}
.de-card { padding: 12px; }
.de-card-selected { outline: 2px solid var(--accent-bg-color); outline-offset: -2px; }
flowboxchild { background: transparent; box-shadow: none; padding: 0; }
flowboxchild:hover, flowboxchild:focus {
  background: transparent;
  box-shadow: none;
  outline: none;
}
";

pub fn build_window(
    app: &adw::Application,
    backends: Backends,
    runtime: tokio::runtime::Handle,
    windowed: bool,
) {
    if let Some(display) = gtk::gdk::Display::default() {
        let provider = gtk::CssProvider::new();
        provider.load_from_string(RAIL_CSS);
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }

    let config = InstallConfig::new_shared();

    let a11y = A11ySettings::new();
    a11y.connect_live_effects(&backends, &runtime);

    let retranslate_hook = steps::new_retranslate_hook();
    let advance_hook = steps::new_advance_hook();
    let step_list: Rc<Vec<Box<dyn Step>>> = Rc::new(steps::build_registry(
        &backends,
        &runtime,
        retranslate_hook.clone(),
        advance_hook.clone(),
        &a11y,
    ));

    // Pages are pushed/popped directly by index below (never by tag), so they
    // must NOT also be registered via `NavigationView::add` — that pushes a
    // page onto the stack immediately, which would fight with our own
    // `push()` calls ("Page is already in navigation stack").
    let nav_view = adw::NavigationView::new();
    let pages: Vec<adw::NavigationPage> = step_list
        .iter()
        .map(|step| adw::NavigationPage::with_tag(&step.widget(), &step.title(), step.id().tag()))
        .collect();
    let pages = Rc::new(pages);

    // Pushed only once the wizard runs past the 9 indexed steps — outside
    // `StepId::ALL`/the step rail entirely. Each carries its own
    // `HeaderBar`, so libadwaita gives it a
    // back chevron for free; the outer prev/next buttons are hidden while
    // either is showing (see the `connect_popped` handler below).
    let progress_page = ProgressPage::new();
    let summary_page = SummaryPage::new(
        backends.clone(),
        runtime.clone(),
        nav_view.clone(),
        progress_page.clone(),
    );

    let rail = gtk::Box::new(gtk::Orientation::Vertical, 4);
    rail.set_margin_top(12);
    rail.set_margin_bottom(12);
    rail.set_margin_start(12);
    rail.set_margin_end(12);
    let mut rail_rows = Vec::with_capacity(step_list.len());
    let mut rail_labels = Vec::with_capacity(step_list.len());
    for step in step_list.iter() {
        let row = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(8)
            .accessible_role(gtk::AccessibleRole::ListItem)
            .build();
        row.add_css_class("step-rail-row");
        row.append(&gtk::Image::from_icon_name(step.icon_name()));
        let label = gtk::Label::new(Some(&step.title()));
        row.append(&label);
        row.update_property(&[gtk::accessible::Property::Label(&step.title())]);
        rail.append(&row);
        rail_rows.push(row);
        rail_labels.push(label);
    }
    let rail_rows = Rc::new(rail_rows);
    let rail_scrolled = gtk::ScrolledWindow::builder().child(&rail).build();
    let sidebar_page = adw::NavigationPage::new(&rail_scrolled, &tr("Steps"));

    let prev_button = gtk::Button::with_label(&tr("Previous"));
    let next_button = gtk::Button::with_label(&tr("Next"));

    let (a11y_button, a11y_rows) = build_a11y_popover(&a11y);

    // Step 7 only — sizing swap/partitions by hand benefits from a
    // calculator at hand; every other step has no use for it. Launched
    // fire-and-forget: `spawn()` just forks+execs, it doesn't block waiting
    // on the child like `run_gparted`'s `.status().await` does, so there's
    // no need to route this through the tokio runtime.
    let calc_button = gtk::Button::from_icon_name("accessories-calculator-symbolic");
    calc_button.set_tooltip_text(Some(&tr("Calculator")));
    calc_button.set_visible(false);
    calc_button.connect_clicked(|_| {
        if let Err(e) = std::process::Command::new("gnome-calculator").spawn() {
            eprintln!("gnome-calculator: failed to launch: {e}");
        }
    });

    let header = adw::HeaderBar::new();
    header.pack_start(&prev_button);
    header.pack_end(&next_button);
    header.pack_end(&a11y_button);
    header.pack_end(&calc_button);

    let toolbar_view = adw::ToolbarView::new();
    toolbar_view.add_top_bar(&header);
    toolbar_view.set_content(Some(&nav_view));
    let content_page = adw::NavigationPage::new(&toolbar_view, "");

    let split_view = adw::NavigationSplitView::new();
    split_view.set_sidebar(Some(&sidebar_page));
    split_view.set_content(Some(&content_page));

    let toast_overlay = adw::ToastOverlay::new();
    toast_overlay.set_child(Some(&split_view));

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .content(&toast_overlay)
        .default_width(1100)
        .default_height(720)
        .title(tr("Modulix OS Installer"))
        .build();

    {
        let toggle_action = gio::SimpleAction::new("toggle-a11y-popover", None);
        let a11y_button = a11y_button.clone();
        toggle_action.connect_activate(move |_, _| {
            a11y_button.popup();
        });
        window.add_action(&toggle_action);
        app.set_accels_for_action("win.toggle-a11y-popover", &["<Ctrl><Alt>a"]);
    }

    {
        let step_list = step_list.clone();
        let rail_labels = rail_labels.clone();
        let rail_rows = rail_rows.clone();
        let sidebar_page = sidebar_page.clone();
        let prev_button = prev_button.clone();
        let next_button = next_button.clone();
        let window = window.clone();
        let a11y_button = a11y_button.clone();
        let a11y_rows = a11y_rows.clone();
        let calc_button = calc_button.clone();
        *retranslate_hook.borrow_mut() = Box::new(move || {
            for ((step, label), row) in step_list
                .iter()
                .zip(rail_labels.iter())
                .zip(rail_rows.iter())
            {
                step.retranslate();
                label.set_label(&step.title());
                row.update_property(&[gtk::accessible::Property::Label(&step.title())]);
            }
            sidebar_page.set_title(&tr("Steps"));
            prev_button.set_label(&tr("Previous"));
            next_button.set_label(&tr("Next"));
            a11y_button.set_tooltip_text(Some(&tr("Accessibility")));
            calc_button.set_tooltip_text(Some(&tr("Calculator")));
            retranslate_a11y_popover(&a11y_rows);
            window.set_title(Some(&tr("Modulix OS Installer")));
        });
    }

    let current_index = Rc::new(Cell::new(0usize));
    let validity_binding: Rc<RefCell<Option<glib::Binding>>> = Rc::new(RefCell::new(None));

    let rebind_validity = {
        let step_list = step_list.clone();
        let next_button = next_button.clone();
        let validity_binding = validity_binding.clone();
        move |idx: usize| {
            if let Some(old) = validity_binding.borrow_mut().take() {
                old.unbind();
            }
            let tracker = step_list[idx].validity();
            let binding = tracker
                .bind_property("is-ready", &next_button, "sensitive")
                .sync_create()
                .build();
            *validity_binding.borrow_mut() = Some(binding);
        }
    };

    let update_rail = {
        let rail_rows = rail_rows.clone();
        let prev_button = prev_button.clone();
        move |idx: usize| {
            for (i, row) in rail_rows.iter().enumerate() {
                if i == idx {
                    row.add_css_class("current-step");
                    row.update_state(&[gtk::accessible::State::Selected(Some(true))]);
                } else {
                    row.remove_css_class("current-step");
                    row.update_state(&[gtk::accessible::State::Selected(Some(false))]);
                }
            }
            prev_button.set_sensitive(idx > 0);
        }
    };

    let update_calc_button = {
        let step_list = step_list.clone();
        let calc_button = calc_button.clone();
        move |idx: usize| {
            calc_button.set_visible(step_list[idx].id() == steps::StepId::Partitioning);
        }
    };

    fn index_for_tag(step_list: &[Box<dyn Step>], tag: &str) -> Option<usize> {
        step_list.iter().position(|step| step.id().tag() == tag)
    }

    {
        // Visible page is authoritative for rail/prev/next/calc state — this
        // one handler covers push, pop, Escape, Alt+Left and swipe-back, so
        // `current_index` can never drift from what's actually on screen.
        let step_list = step_list.clone();
        let current_index = current_index.clone();
        let rebind_validity = rebind_validity.clone();
        let update_rail = update_rail.clone();
        let update_calc_button = update_calc_button.clone();
        let prev_button = prev_button.clone();
        let next_button = next_button.clone();
        let calc_button = calc_button.clone();
        nav_view.connect_notify_local(Some("visible-page"), move |nav_view, _| {
            let Some(visible) = nav_view.visible_page() else {
                return;
            };
            match visible
                .tag()
                .and_then(|tag| index_for_tag(&step_list, &tag))
            {
                Some(idx) => {
                    current_index.set(idx);
                    rebind_validity(idx);
                    update_rail(idx);
                    update_calc_button(idx);
                    prev_button.set_visible(true);
                    next_button.set_visible(step_list[idx].shows_next());
                }
                None => {
                    prev_button.set_visible(false);
                    next_button.set_visible(false);
                    calc_button.set_visible(false);
                }
            }
        });
    }

    nav_view.push(&pages[0]);

    {
        let nav_view = nav_view.clone();
        prev_button.connect_clicked(move |_| {
            nav_view.pop();
        });
    }

    {
        let advance: Rc<dyn Fn()> = Rc::new({
            let current_index = current_index.clone();
            let nav_view = nav_view.clone();
            let pages = pages.clone();
            let step_list = step_list.clone();
            let config = config.clone();
            let toast_overlay = toast_overlay.clone();
            let summary_page = summary_page.clone();
            move || {
                let idx = current_index.get();
                if let Err(e) = step_list[idx].commit(&mut config.borrow_mut()) {
                    toast_overlay.add_toast(adw::Toast::new(&format!(
                        "{}: {e}",
                        tr("Couldn't save this step")
                    )));
                    return;
                }

                let mut next_idx = idx + 1;
                while next_idx < step_list.len()
                    && !step_list[next_idx].is_relevant(&config.borrow())
                {
                    next_idx += 1;
                }

                if next_idx >= step_list.len() {
                    summary_page.refresh(&config.borrow());
                    nav_view.push(&summary_page.page());
                    return;
                }

                nav_view.push(&pages[next_idx]);
            }
        });
        next_button.connect_clicked({
            let advance = advance.clone();
            move |_| advance()
        });
        *advance_hook.borrow_mut() = Box::new(move || advance());
    }

    if !windowed {
        window.fullscreen();
    }
    window.present();
}

/// Rows bound to the same [`A11ySettings`] instance as the accessibility
/// step (step 1, which also carries the narrator toggle) — toggling one
/// here or there stays in sync everywhere, no manual resynchronization needed.
#[derive(Clone)]
struct A11yPopoverRows {
    narrator: adw::SwitchRow,
    high_contrast: adw::SwitchRow,
    large_text: adw::SwitchRow,
    screen_magnifier: adw::SwitchRow,
    sticky_keys: adw::SwitchRow,
}

fn popover_row(title: String, settings: &A11ySettings, property: &str) -> adw::SwitchRow {
    let row = adw::SwitchRow::builder().title(title).build();
    row.bind_property("active", settings, property)
        .bidirectional()
        .sync_create()
        .build();
    row
}

/// Header-bar `MenuButton` reachable from every page — the only place the
/// accessibility toggles are reachable from once the user has moved past
/// step 1 without navigating back.
fn build_a11y_popover(a11y: &A11ySettings) -> (gtk::MenuButton, A11yPopoverRows) {
    let rows = A11yPopoverRows {
        narrator: popover_row(tr("Enable narrator"), a11y, "narrator"),
        high_contrast: popover_row(tr("High contrast"), a11y, "high-contrast"),
        large_text: popover_row(tr("Large text"), a11y, "large-text"),
        screen_magnifier: popover_row(tr("Screen magnifier"), a11y, "screen-magnifier"),
        sticky_keys: popover_row(tr("Sticky keys"), a11y, "sticky-keys"),
    };

    let list = gtk::ListBox::new();
    list.add_css_class("boxed-list");
    list.set_selection_mode(gtk::SelectionMode::None);
    list.append(&rows.narrator);
    list.append(&rows.high_contrast);
    list.append(&rows.large_text);
    list.append(&rows.screen_magnifier);
    list.append(&rows.sticky_keys);
    list.set_margin_top(6);
    list.set_margin_bottom(6);
    list.set_margin_start(6);
    list.set_margin_end(6);

    let popover = gtk::Popover::builder().child(&list).build();

    let button = gtk::MenuButton::builder()
        .icon_name("accessibility-symbolic")
        .tooltip_text(tr("Accessibility"))
        .popover(&popover)
        .build();

    (button, rows)
}

fn retranslate_a11y_popover(rows: &A11yPopoverRows) {
    rows.narrator.set_title(&tr("Enable narrator"));
    rows.high_contrast.set_title(&tr("High contrast"));
    rows.large_text.set_title(&tr("Large text"));
    rows.screen_magnifier.set_title(&tr("Screen magnifier"));
    rows.sticky_keys.set_title(&tr("Sticky keys"));
}
