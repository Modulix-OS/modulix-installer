//! Fullscreen wizard window: `NavigationSplitView` (step rail | content),
//! content = `ToolbarView` (header + prev/next) wrapping a `NavigationView`
//! that push/pops between the 9 step pages already built by
//! [`crate::steps::build_registry`].

use crate::backend::Backends;
use crate::config::InstallConfig;
use crate::i18n::tr;
use crate::steps::{self, Step};
use adw::prelude::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

const RAIL_CSS: &str = "
.step-rail-row { padding: 6px 10px; border-radius: 6px; }
.step-rail-row.current-step { font-weight: bold; background-color: alpha(currentColor, 0.08); }
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

    let retranslate_hook = steps::new_retranslate_hook();
    let step_list: Rc<Vec<Box<dyn Step>>> = Rc::new(steps::build_registry(
        &backends,
        &runtime,
        retranslate_hook.clone(),
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

    let rail = gtk::Box::new(gtk::Orientation::Vertical, 4);
    rail.set_margin_top(12);
    rail.set_margin_bottom(12);
    rail.set_margin_start(12);
    rail.set_margin_end(12);
    let mut rail_rows = Vec::with_capacity(step_list.len());
    let mut rail_labels = Vec::with_capacity(step_list.len());
    for step in step_list.iter() {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        row.add_css_class("step-rail-row");
        row.append(&gtk::Image::from_icon_name(step.icon_name()));
        let label = gtk::Label::new(Some(&step.title()));
        row.append(&label);
        rail.append(&row);
        rail_rows.push(row);
        rail_labels.push(label);
    }
    let rail_rows = Rc::new(rail_rows);
    let rail_scrolled = gtk::ScrolledWindow::builder().child(&rail).build();
    let sidebar_page = adw::NavigationPage::new(&rail_scrolled, &tr("Steps"));

    let prev_button = gtk::Button::with_label(&tr("Previous"));
    let next_button = gtk::Button::with_label(&tr("Next"));

    let header = adw::HeaderBar::new();
    header.pack_start(&prev_button);
    header.pack_end(&next_button);

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
        let step_list = step_list.clone();
        let rail_labels = rail_labels.clone();
        let sidebar_page = sidebar_page.clone();
        let prev_button = prev_button.clone();
        let next_button = next_button.clone();
        let window = window.clone();
        *retranslate_hook.borrow_mut() = Box::new(move || {
            for (step, label) in step_list.iter().zip(rail_labels.iter()) {
                step.retranslate();
                label.set_label(&step.title());
            }
            sidebar_page.set_title(&tr("Steps"));
            prev_button.set_label(&tr("Previous"));
            next_button.set_label(&tr("Next"));
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
                } else {
                    row.remove_css_class("current-step");
                }
            }
            prev_button.set_sensitive(idx > 0);
        }
    };

    nav_view.push(&pages[0]);
    rebind_validity(0);
    update_rail(0);

    {
        let current_index = current_index.clone();
        let rebind_validity = rebind_validity.clone();
        let update_rail = update_rail.clone();
        let nav_view = nav_view.clone();
        prev_button.connect_clicked(move |_| {
            let idx = current_index.get();
            if idx == 0 {
                return;
            }
            if nav_view.pop() {
                let new_idx = idx - 1;
                current_index.set(new_idx);
                rebind_validity(new_idx);
                update_rail(new_idx);
            }
        });
    }

    {
        let current_index = current_index.clone();
        let rebind_validity = rebind_validity.clone();
        let update_rail = update_rail.clone();
        let nav_view = nav_view.clone();
        let pages = pages.clone();
        let step_list = step_list.clone();
        let config = config.clone();
        let toast_overlay = toast_overlay.clone();
        next_button.connect_clicked(move |_| {
            let idx = current_index.get();
            if let Err(e) = step_list[idx].commit(&mut config.borrow_mut()) {
                toast_overlay.add_toast(adw::Toast::new(&format!(
                    "{}: {e}",
                    tr("Couldn't save this step")
                )));
                return;
            }

            let mut next_idx = idx + 1;
            while next_idx < step_list.len() && !step_list[next_idx].is_relevant(&config.borrow()) {
                next_idx += 1;
            }

            if next_idx >= step_list.len() {
                toast_overlay.add_toast(adw::Toast::new(&tr(
                    "Setup isn't implemented yet — that's iteration 2",
                )));
                return;
            }

            nav_view.push(&pages[next_idx]);
            current_index.set(next_idx);
            rebind_validity(next_idx);
            update_rail(next_idx);
        });
    }

    if !windowed {
        window.fullscreen();
    }
    window.present();
}
