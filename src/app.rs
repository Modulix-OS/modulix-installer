//! Fullscreen wizard window: `NavigationSplitView` (step rail | content),
//! content = `ToolbarView` (header + prev/next) wrapping a `NavigationView`
//! that navigates between the 9 step pages already built by
//! [`crate::steps::build_registry`], plus the review and install pages from
//! [`crate::finish`].
//!
//! The rail is the navigation control, not a read-out: its 11 rows are buttons
//! going through `navigate_to`, which rebuilds the whole navigation stack in one
//! `adw::NavigationView::replace` and is also what the "Next" button and the
//! [`crate::steps::AdvanceHook`] call. Rows reachable only forwards are dimmed
//! until every step before them validates, and all of them are locked for good
//! once the install starts.

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
.step-rail-row:disabled { opacity: 0.5; }
.mx-warning-strip {
  background-color: var(--warning-bg-color);
  color: var(--warning-fg-color);
  border-radius: 12px;
  padding: 12px;
}
.de-card { padding: 12px; }
.de-card:checked { background-color: var(--card-bg-color); }
.de-card-selected { outline: 2px solid var(--accent-bg-color); outline-offset: -2px; }
flowboxchild { background: transparent; box-shadow: none; padding: 0; }
flowboxchild:hover, flowboxchild:focus {
  background: transparent;
  box-shadow: none;
  outline: none;
}
";

/// Root-stack child names: the pre-wizard welcome page, and everything else.
const WELCOME_STACK_NAME: &str = "welcome";
const WIZARD_STACK_NAME: &str = "wizard";

/// Adwaita symbolic icon of the step rail's review row, which is the summary
/// page (`crate::finish::SUMMARY_TAG`).
const REVIEW_ICON: &str = "object-select-symbolic";
/// Adwaita symbolic icon of the step rail's install row, which is the progress
/// page (`crate::finish::PROGRESS_TAG`).
const INSTALL_ICON: &str = "system-software-install-symbolic";

/// One row of the step rail, i.e. one place the wizard can be.
///
/// The 9 wizard steps are positional indices into the registry
/// `crate::steps::build_registry` returns — the same positions `pages`,
/// `rail_rows` and `rail_labels` are built from. The review and install pages
/// live outside `StepId` on purpose (`StepId::ALL` stays at 9) and are keyed on
/// their `adw::NavigationPage` tags instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RailTarget {
    Step(usize),
    Review,
    Install,
}

impl RailTarget {
    /// Position of this target in wizard order, so two targets can be compared
    /// without matching every pair.
    ///
    /// * `step_count` - number of steps in the registry (9 today).
    ///
    /// # Post-conditions
    /// A `Step(i)` yields `i`, the review page `step_count` and the install
    /// page `step_count + 1`; the result is also the row's index in
    /// `rail_rows`/`rail_labels`.
    fn order(self, step_count: usize) -> usize {
        match self {
            RailTarget::Step(i) => i,
            RailTarget::Review => step_count,
            RailTarget::Install => step_count + 1,
        }
    }
}

/// Fingerprint of the partitioning answers, used to decide whether the
/// confirmation dialog of the partitioning step has already been acknowledged
/// *for this layout*.
///
/// * `cfg` - the shared install config, after the partitioning step committed.
///
/// # Post-conditions
/// Two configs yielding the same string describe the same layout. The
/// passphrase and the TPM2 PIN are deliberately left out — they are not part of
/// the layout, and `Secret`'s `Debug` prints them masked anyway, so including
/// them would change nothing but suggest otherwise.
fn partitioning_fingerprint(cfg: &InstallConfig) -> String {
    let p = &cfg.partitioning;
    format!(
        "{:?}|{:?}|{:?}|{:?}|{:?}|{}|{}|{:?}",
        p.mode,
        p.target_disk,
        p.shrink_to_bytes,
        p.selected_free_space_id,
        p.swap_mode,
        p.encryption_enabled,
        p.tpm2_enabled,
        p.manual
    )
}

/// Builds and presents the installer window, fullscreen.
///
/// * `app` - the running application.
/// * `backends` - subsystem backends every step and task acts through.
/// * `runtime` - tokio handle backend work is spawned on.
pub fn build_window(app: &adw::Application, backends: Backends, runtime: tokio::runtime::Handle) {
    if let Some(display) = gtk::gdk::Display::default() {
        let provider = gtk::CssProvider::new();
        provider.load_from_string(&format!("{RAIL_CSS}{}", crate::welcome::CSS));
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
    let select_language_hook = steps::new_select_language_hook();
    let step_list: Rc<Vec<Box<dyn Step>>> = Rc::new(steps::build_registry(
        &backends,
        &runtime,
        retranslate_hook.clone(),
        advance_hook.clone(),
        select_language_hook.clone(),
        &a11y,
    ));

    // Pages are pushed/replaced by index below, so they must NOT also be
    // registered via `NavigationView::add` — that pushes a page onto the stack
    // immediately, which would fight with our own calls ("Page is already in
    // navigation stack"). Their tags are read-only, for `slot_for_tag`.
    let nav_view = adw::NavigationView::new();
    let pages: Vec<adw::NavigationPage> = step_list
        .iter()
        .map(|step| adw::NavigationPage::with_tag(&step.widget(), &step.title(), step.id().tag()))
        .collect();
    let pages = Rc::new(pages);

    // Outside `StepId::ALL` but not outside the rail: each owns one of its two
    // last rows, keyed on the page tag rather than on a `StepId` (see
    // `slot_for_tag`). Neither carries a `HeaderBar` of its own, so the outer
    // prev/next buttons are what the `visible-page` handler below adjusts while
    // either is showing.
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
    // 11 rows: the 9 steps, then the review and install pages. A `Button`
    // rather than a `Box` so a row is clickable, reachable by keyboard, dimmed
    // on its own when insensitive, and published to Orca as a button — none of
    // which a `GestureClick` on a box would give.
    let rail_targets: Vec<RailTarget> = step_list
        .iter()
        .enumerate()
        .map(|(i, _)| RailTarget::Step(i))
        .chain([RailTarget::Review, RailTarget::Install])
        .collect();
    let mut rail_rows = Vec::with_capacity(rail_targets.len());
    let mut rail_labels = Vec::with_capacity(rail_targets.len());
    for target in rail_targets.iter() {
        let (icon, title) = match *target {
            RailTarget::Step(i) => (step_list[i].icon_name(), step_list[i].title()),
            RailTarget::Review => (REVIEW_ICON, tr("Ready to install")),
            RailTarget::Install => (INSTALL_ICON, tr("Installing")),
        };
        let content = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        content.append(&gtk::Image::from_icon_name(icon));
        let label = gtk::Label::builder().label(&title).xalign(0.0).build();
        content.append(&label);
        let row = gtk::Button::builder()
            .child(&content)
            .css_classes(["flat", "step-rail-row"])
            .build();
        row.update_property(&[gtk::accessible::Property::Label(&title)]);
        rail.append(&row);
        rail_rows.push(row);
        rail_labels.push(label);
    }
    let rail_targets = Rc::new(rail_targets);
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

    // Nothing can act on a minimise/maximise/close button: the session is a
    // single fullscreen app under a sway kiosk with no window decorations and
    // no way back to a desktop.
    let header = adw::HeaderBar::new();
    header.set_show_start_title_buttons(false);
    header.set_show_end_title_buttons(false);
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

    // The welcome page comes before the wizard and must show none of it: no
    // step rail, no header bar, no prev/next. A `Stack` ahead of the whole
    // `NavigationSplitView` is the only placement that gives that — a page
    // inside `nav_view` would still sit under the header and beside the rail.
    let root_stack = gtk::Stack::builder()
        .transition_type(gtk::StackTransitionType::Crossfade)
        .transition_duration(400)
        .build();
    // The greeting the user clicked is the language they asked for: apply it
    // through the language step (dropdown, `InstallConfig::language`,
    // retranslate pass, keyboard default) *before* revealing the wizard, so its
    // first frame is already translated. The retranslate hook is installed
    // further down, long before any click can happen.
    let welcome_page = crate::welcome::build_page({
        let root_stack = root_stack.clone();
        let select_language_hook = select_language_hook.clone();
        move |locale: &str| {
            (select_language_hook.borrow())(locale);
            root_stack.set_visible_child_name(WIZARD_STACK_NAME);
        }
    });
    root_stack.add_named(&welcome_page, Some(WELCOME_STACK_NAME));
    root_stack.add_named(&split_view, Some(WIZARD_STACK_NAME));
    root_stack.set_visible_child_name(WELCOME_STACK_NAME);

    let toast_overlay = adw::ToastOverlay::new();
    toast_overlay.set_child(Some(&root_stack));

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
        let summary_page = summary_page.clone();
        let progress_page = progress_page.clone();
        let content_page = content_page.clone();
        let nav_view_for_title = nav_view.clone();
        let pages_for_title = pages.clone();
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
            // The rail's last two rows are the review and install pages, which
            // the zip above stops short of — they are not steps.
            for (target, title) in [
                (RailTarget::Review, tr("Ready to install")),
                (RailTarget::Install, tr("Installing")),
            ] {
                let idx = target.order(step_list.len());
                rail_labels[idx].set_label(&title);
                rail_rows[idx].update_property(&[gtk::accessible::Property::Label(&title)]);
            }
            // Step pages carry no header of their own; their title is what the
            // single outer header bar displays, so it has to be re-set too —
            // and then re-read for whichever page is currently visible.
            for (step, page) in step_list.iter().zip(pages_for_title.iter()) {
                page.set_title(&step.title());
            }
            summary_page.retranslate();
            progress_page.retranslate();
            if let Some(visible) = nav_view_for_title.visible_page() {
                content_page.set_title(&visible.title());
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

    let current_slot = Rc::new(Cell::new(RailTarget::Step(0)));
    // Set the first time the install-progress page is reached: the pipeline is
    // destructive and nothing it has written is undone by walking back, so
    // every rail row stays locked while it is set. The failure page's "Change
    // the configuration" button is the only thing that clears it (see
    // `set_reconfigure_hook` below); a successful install never does.
    let installing = Rc::new(Cell::new(false));
    // Furthest `RailTarget::order` ever shown. A row past it is locked even
    // when every step before it is valid: the user has to have *been* on the
    // previous step to reach the next one, so the rail can only ever run one
    // row ahead of what has been seen, and a row already visited stays
    // reachable afterwards.
    let visited_max = Rc::new(Cell::new(0usize));
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

    // Marks the current row and decides which rows may be jumped to: backwards
    // always, forwards only once every relevant step before the row reports
    // `is-ready` (a forward jump commits the steps it walks over) *and* only up
    // to one row past `visited_max`, so an unvisited step keeps the one after it
    // locked. Called from the `visible-page` handler and from every step's
    // `is-ready` signal, so a field becoming valid un-dims the row below it
    // without navigating.
    let refresh_rail: Rc<dyn Fn()> = Rc::new({
        let rail_rows = rail_rows.clone();
        let rail_targets = rail_targets.clone();
        let step_list = step_list.clone();
        let config = config.clone();
        let current_slot = current_slot.clone();
        let installing = installing.clone();
        let visited_max = visited_max.clone();
        let prev_button = prev_button.clone();
        move || {
            let step_count = step_list.len();
            let current = current_slot.get();
            let current_order = current.order(step_count);
            let locked = installing.get();
            let reach = visited_max.get() + 1;
            let mut ready_before = true;
            for (row, target) in rail_rows.iter().zip(rail_targets.iter()) {
                let target = *target;
                let order = target.order(step_count);
                let relevant = match target {
                    RailTarget::Step(i) => step_list[i].is_relevant(&config.borrow()),
                    _ => true,
                };
                // The install page is entered by the summary's own "Install"
                // button and by nothing else — never by a rail click.
                let reachable = relevant
                    && target != RailTarget::Install
                    && (order <= current_order || (ready_before && order <= reach));
                row.set_sensitive(!locked && reachable);
                if target == current {
                    row.add_css_class("current-step");
                    row.update_state(&[gtk::accessible::State::Selected(Some(true))]);
                } else {
                    row.remove_css_class("current-step");
                    row.update_state(&[gtk::accessible::State::Selected(Some(false))]);
                }
                if let RailTarget::Step(i) = target
                    && relevant
                {
                    ready_before = ready_before && step_list[i].validity().is_ready();
                }
            }
            prev_button.set_sensitive(!locked && current_order > 0);
        }
    });

    let update_calc_button = {
        let step_list = step_list.clone();
        let calc_button = calc_button.clone();
        move |idx: usize| {
            calc_button.set_visible(step_list[idx].id() == steps::StepId::Partitioning);
        }
    };

    /// Rail row a visible `adw::NavigationPage` belongs to, by tag.
    ///
    /// * `step_list` - the step registry, in rail order.
    /// * `tag` - the visible page's tag.
    ///
    /// # Post-conditions
    /// `None` only for a page that carries no tag at all; the 9 steps match
    /// `StepId::tag`, the two finish pages their `crate::finish` constants.
    fn slot_for_tag(step_list: &[Box<dyn Step>], tag: &str) -> Option<RailTarget> {
        match tag {
            crate::finish::SUMMARY_TAG => Some(RailTarget::Review),
            crate::finish::PROGRESS_TAG => Some(RailTarget::Install),
            _ => step_list
                .iter()
                .position(|step| step.id().tag() == tag)
                .map(RailTarget::Step),
        }
    }

    {
        // Visible page is authoritative for rail/prev/next/calc state — this
        // one handler covers push, replace, pop, Escape, Alt+Left and
        // swipe-back, so `current_slot` can never drift from what's actually on
        // screen.
        let step_list = step_list.clone();
        let current_slot = current_slot.clone();
        let installing = installing.clone();
        let visited_max = visited_max.clone();
        let rebind_validity = rebind_validity.clone();
        let refresh_rail = refresh_rail.clone();
        let update_calc_button = update_calc_button.clone();
        let prev_button = prev_button.clone();
        let next_button = next_button.clone();
        let calc_button = calc_button.clone();
        let content_page = content_page.clone();
        nav_view.connect_notify_local(Some("visible-page"), move |nav_view, _| {
            let Some(visible) = nav_view.visible_page() else {
                return;
            };
            // The summary and progress pages have no header bar of their own,
            // so the only title on screen is the outer one — it has to follow
            // whatever page is showing.
            content_page.set_title(&visible.title());
            let slot = visible.tag().and_then(|tag| slot_for_tag(&step_list, &tag));
            if let Some(target) = slot {
                visited_max.set(visited_max.get().max(target.order(step_list.len())));
            }
            match slot {
                Some(RailTarget::Step(idx)) => {
                    current_slot.set(RailTarget::Step(idx));
                    rebind_validity(idx);
                    update_calc_button(idx);
                    prev_button.set_visible(true);
                    next_button.set_visible(step_list[idx].shows_next());
                }
                slot => {
                    // Summary and progress. The back chevron used to come from
                    // their own header bar; with that gone, the outer
                    // "Previous" takes over — but only where going back makes
                    // sense, which `can_pop` already answers (it is false on
                    // the progress page).
                    if let Some(target) = slot {
                        current_slot.set(target);
                        if target == RailTarget::Install {
                            installing.set(true);
                        }
                    }
                    prev_button.set_visible(visible.can_pop());
                    next_button.set_visible(false);
                    calc_button.set_visible(false);
                }
            }
            refresh_rail();
        });
    }

    // Jumping to a rail row, in either direction. Everything but the
    // partitioning confirmation below lives here; `navigate_to` is what the
    // rail rows, the "Next" button and the `AdvanceHook` actually call.
    let perform_navigation: Rc<dyn Fn(RailTarget)> = Rc::new({
        let nav_view = nav_view.clone();
        let pages = pages.clone();
        let step_list = step_list.clone();
        let config = config.clone();
        let toast_overlay = toast_overlay.clone();
        let summary_page = summary_page.clone();
        let current_slot = current_slot.clone();
        let installing = installing.clone();
        let visited_max = visited_max.clone();
        move |target: RailTarget| {
            if installing.get() || target == RailTarget::Install {
                return;
            }
            let step_count = step_list.len();
            let current = current_slot.get();
            if target == current {
                return;
            }
            // Same one-row-past-`visited_max` ceiling `refresh_rail` dims on;
            // this only guards the gap between a click and a refresh.
            if target.order(step_count) > visited_max.get() + 1 {
                return;
            }
            // Every step strictly before the target, so `n` for the review page
            // and `i` for `Step(i)`.
            let steps_before = target.order(step_count).min(step_count);
            let current_order = current.order(step_count);

            if steps_before > current_order {
                for i in current_order..steps_before {
                    if !step_list[i].is_relevant(&config.borrow()) {
                        continue;
                    }
                    // Its rail row is insensitive while a step is not ready, so
                    // this only guards the gap between a click and a refresh.
                    if !step_list[i].validity().is_ready() {
                        return;
                    }
                    if let Err(e) = step_list[i].commit(&mut config.borrow_mut()) {
                        toast_overlay.add_toast(adw::Toast::new(&format!(
                            "{}: {e}",
                            tr("Couldn't save this step")
                        )));
                        return;
                    }
                }
            }

            // One `replace` instead of a push/pop sequence: it serves both
            // directions, cannot raise "Page is already in navigation stack",
            // and notifies `visible-page` once — so `current_slot` stays
            // derived from the handler above rather than written here.
            let mut stack: Vec<adw::NavigationPage> = (0..steps_before)
                .filter(|&i| step_list[i].is_relevant(&config.borrow()))
                .map(|i| pages[i].clone())
                .collect();
            match target {
                RailTarget::Step(i) => stack.push(pages[i].clone()),
                RailTarget::Review => {
                    summary_page.refresh(&config.borrow());
                    stack.push(summary_page.page());
                }
                RailTarget::Install => return,
            }
            nav_view.replace(&stack);
        }
    });

    // Leaving the partitioning step forward is the one move that asks for a
    // confirmation: it is the only step whose answers destroy data, and the
    // summary's own destructive dialog comes much later, after 3 more pages.
    // Asked once per layout, not once per session — the acknowledged
    // fingerprint is kept, so editing the layout and leaving again asks again.
    let navigate_to: Rc<dyn Fn(RailTarget)> = Rc::new({
        let perform = perform_navigation.clone();
        let step_list = step_list.clone();
        let config = config.clone();
        let toast_overlay = toast_overlay.clone();
        let window = window.clone();
        let current_slot = current_slot.clone();
        let acknowledged: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
        move |target: RailTarget| {
            let step_count = step_list.len();
            let RailTarget::Step(idx) = current_slot.get() else {
                perform(target);
                return;
            };
            if step_list[idx].id() != steps::StepId::Partitioning || target.order(step_count) <= idx
            {
                perform(target);
                return;
            }
            // Same guard `perform` applies before committing anything: an
            // invalid layout must not reach the config, not even to be
            // fingerprinted.
            if !step_list[idx].validity().is_ready() {
                return;
            }
            // The dialog reports what is on the page, so the step has to be
            // committed before it is built. `perform` commits it again, which
            // is idempotent — `commit` only copies widget state into the
            // config.
            if let Err(e) = step_list[idx].commit(&mut config.borrow_mut()) {
                toast_overlay.add_toast(adw::Toast::new(&format!(
                    "{}: {e}",
                    tr("Couldn't save this step")
                )));
                return;
            }
            let fingerprint = partitioning_fingerprint(&config.borrow());
            if acknowledged.borrow().as_deref() == Some(fingerprint.as_str()) {
                perform(target);
                return;
            }

            let disk = config
                .borrow()
                .partitioning
                .target_disk
                .clone()
                .unwrap_or_default();
            let dialog = adw::AlertDialog::builder()
                .heading(tr("Confirm the partitioning"))
                .body(format!(
                    "{}\n\n{}: {}",
                    tr("This layout is what the installation will write to disk. Nothing is erased before you confirm it on the summary page, and you can come back to this step until then."),
                    tr("Target disk"),
                    disk
                ))
                .build();
            dialog.add_response("cancel", &tr("Cancel"));
            dialog.add_response("continue", &tr("Continue"));
            dialog.set_default_response(Some("continue"));
            dialog.set_close_response("cancel");

            let perform = perform.clone();
            let acknowledged = acknowledged.clone();
            let window = window.clone();
            glib::spawn_future_local(async move {
                if dialog.choose_future(Some(&window)).await != "continue" {
                    return;
                }
                *acknowledged.borrow_mut() = Some(fingerprint);
                perform(target);
            });
        }
    });

    for (row, target) in rail_rows.iter().zip(rail_targets.iter()) {
        let navigate_to = navigate_to.clone();
        let target = *target;
        row.connect_clicked(move |_| navigate_to(target));
    }

    // Failure page only: the one thing that clears `installing` and lets the
    // rail work again. It goes back to the review page rather than to a step,
    // because from there every row is already visited and therefore reachable,
    // so the user picks what to change. `perform_navigation` is called directly
    // (not `navigate_to`): this is a backwards move, nothing to commit and no
    // partitioning dialog to raise.
    //
    // The dialog is not a formality: a failed pipeline stopped wherever it
    // stopped and `PostInstallTask` never ran, so partitions, filesystems,
    // open LUKS mappers, the `/mnt` mounts and the target's swap are all still
    // in whatever state the failure left them — going back changes none of
    // that, and a second run may well fail on it.
    progress_page.set_reconfigure_hook({
        let installing = installing.clone();
        let perform = perform_navigation.clone();
        let window = window.clone();
        move || {
            let dialog = adw::AlertDialog::builder()
                .heading(tr("Change the configuration"))
                .body(tr(
                    "The failed install is not undone: partitions, filesystems and unlocked encrypted volumes stay as it left them, so starting again may fail on them. Restart the computer instead if you want a clean target.",
                ))
                .build();
            dialog.add_response("cancel", &tr("Cancel"));
            dialog.add_response("back", &tr("Change the configuration"));
            dialog.set_default_response(Some("cancel"));
            dialog.set_close_response("cancel");

            let installing = installing.clone();
            let perform = perform.clone();
            let window = window.clone();
            glib::spawn_future_local(async move {
                if dialog.choose_future(Some(&window)).await != "back" {
                    return;
                }
                installing.set(false);
                perform(RailTarget::Review);
            });
        }
    });

    // A step becoming valid (or invalid) changes which rows are reachable, and
    // that must show without a navigation.
    for step in step_list.iter() {
        let refresh_rail = refresh_rail.clone();
        step.validity()
            .connect_notify_local(Some("is-ready"), move |_, _| refresh_rail());
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
            let step_list = step_list.clone();
            let config = config.clone();
            let current_slot = current_slot.clone();
            let navigate_to = navigate_to.clone();
            move || {
                let RailTarget::Step(idx) = current_slot.get() else {
                    return;
                };
                let mut next_idx = idx + 1;
                while next_idx < step_list.len()
                    && !step_list[next_idx].is_relevant(&config.borrow())
                {
                    next_idx += 1;
                }
                // The commit of the step being left, the summary refresh and
                // the stack rebuild all live in `navigate_to`.
                navigate_to(if next_idx >= step_list.len() {
                    RailTarget::Review
                } else {
                    RailTarget::Step(next_idx)
                });
            }
        });
        next_button.connect_clicked({
            let advance = advance.clone();
            move |_| advance()
        });
        *advance_hook.borrow_mut() = Box::new(move || advance());
    }

    window.fullscreen();
    window.present();
}

/// Builds and presents the only window shown when a backend could not be
/// reached: a fullscreen error page whose single action is to quit.
///
/// A dialog would not do. An `adw::AlertDialog` needs a parent window, so it
/// could only ever appear on top of a wizard that has no working backend
/// behind it — which is exactly how a failed connection used to end up
/// simulating a whole install.
///
/// * `app` - the running application; quitting it is the only way out.
/// * `reason` - the full, already-rendered failure message.
///
/// # Post-conditions
/// The message is shown in full: selectable, wrapped on word or character
/// boundaries (D-Bus errors and device paths carry no spaces) and scrollable
/// rather than clipped.
pub fn build_fatal_window(app: &adw::Application, reason: &str) {
    let message = gtk::Label::builder()
        .label(reason)
        .selectable(true)
        .wrap(true)
        .wrap_mode(gtk::pango::WrapMode::WordChar)
        .xalign(0.0)
        .css_classes(["monospace"])
        .build();
    let scroller = gtk::ScrolledWindow::builder()
        .child(&message)
        .propagate_natural_height(true)
        .max_content_height(360)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .build();
    scroller.add_css_class("card");

    let quit = gtk::Button::builder()
        .label(tr("Quit"))
        .halign(gtk::Align::Center)
        .css_classes(["destructive-action", "pill"])
        .build();

    let body = gtk::Box::new(gtk::Orientation::Vertical, 18);
    body.append(&scroller);
    body.append(&quit);

    let status = adw::StatusPage::builder()
        .icon_name("dialog-error-symbolic")
        .title(tr("The installer cannot start"))
        .description(tr("A required system service could not be reached."))
        .child(&body)
        .build();

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&adw::HeaderBar::new());
    toolbar.set_content(Some(&status));

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title(tr("Modulix OS Installer"))
        .content(&toolbar)
        .build();

    quit.connect_clicked({
        let app = app.clone();
        move |_| app.quit()
    });

    window.fullscreen();
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
