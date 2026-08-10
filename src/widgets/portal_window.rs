//! Captive-portal sign-in window (network step). Entirely offline in `--fake`
//! (see [`builtin_portal_html`]); in the real backend it loads NM's own
//! connectivity-check URI, which is what actually triggers a captive portal's
//! redirect.
//!
//! Security note (root-in-kiosk): this embeds a third-party-controlled web
//! page (any café's captive portal) in a process
//! running as root inside the kiosk. WebKitGTK 6.0 always sandboxes its web
//! process via bubblewrap and the network session below is ephemeral
//! (no persisted cookies/cache), but the risk is real — verify on the ISO
//! that user namespaces are available, or WebKit's web process won't start.

use crate::backend::net::{NetworkBackend, PortalPage};
use crate::bridge;
use crate::i18n::tr;
use adw::prelude::*;
use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;
use webkit6::prelude::*;

/// `.invalid` is reserved by RFC 2606 and will never resolve, and
/// `decide-policy` fires *before* any load starts — `.ignore()` guarantees no
/// socket is ever opened, so the whole flow stays offline in `--fake`. A
/// custom URI scheme risked being rejected by WebKit before reaching the
/// policy handler at all.
const PORTAL_SENTINEL_URI: &str = "http://modulix.invalid/portal-accepted";

pub struct PortalWindow {
    dialog: adw::Dialog,
    tick: Rc<Cell<Option<glib::SourceId>>>,
}

impl PortalWindow {
    pub fn present(
        parent: &impl IsA<gtk::Widget>,
        backend: Arc<dyn NetworkBackend>,
        runtime: tokio::runtime::Handle,
        page: PortalPage,
    ) -> Self {
        // A fresh, in-memory session: the live ISO must leave no cookie or
        // cache behind, and starting clean guarantees the login page shows up
        // instead of a stale "already signed in" state.
        let session = webkit6::NetworkSession::new_ephemeral();
        let view = webkit6::WebView::builder()
            .network_session(&session)
            .build();
        // Fully qualified: `gtk::prelude::WidgetExt` also has a `settings()`
        // method (the shared `GtkSettings` singleton) that would otherwise
        // shadow WebKit's own per-view settings object.
        if let Some(settings) = webkit6::prelude::WebViewExt::settings(&view) {
            settings.set_enable_dns_prefetching(false);
            settings.set_enable_developer_extras(false);
            settings.set_javascript_can_open_windows_automatically(false);
        }

        {
            let backend = backend.clone();
            let runtime = runtime.clone();
            view.connect_decide_policy(move |_view, decision, kind| {
                if kind != webkit6::PolicyDecisionType::NavigationAction {
                    return false;
                }
                let Some(nav) = decision.downcast_ref::<webkit6::NavigationPolicyDecision>() else {
                    return false;
                };
                let uri = nav
                    .navigation_action()
                    .and_then(|action| action.request())
                    .and_then(|request| request.uri());
                if uri.as_deref() == Some(PORTAL_SENTINEL_URI) {
                    decision.ignore();
                    let backend = backend.clone();
                    bridge::spawn(
                        &runtime,
                        async move { backend.complete_portal().await },
                        |_| {},
                    );
                    return true;
                }
                false
            });
        }

        let reload_button = gtk::Button::from_icon_name("view-refresh-symbolic");
        reload_button.set_tooltip_text(Some(&tr("Reload")));
        reload_button.update_property(&[gtk::accessible::Property::Label(&tr("Reload"))]);
        {
            let view = view.clone();
            reload_button.connect_clicked(move |_| view.reload());
        }

        let header = adw::HeaderBar::new();
        header.pack_end(&reload_button);

        let toolbar_view = adw::ToolbarView::new();
        toolbar_view.add_top_bar(&header);
        toolbar_view.set_content(Some(&view));

        let dialog = adw::Dialog::builder()
            .title(tr("Wi-Fi sign-in"))
            .content_width(900)
            .content_height(700)
            .child(&toolbar_view)
            .build();

        let tick_cell: Rc<Cell<Option<glib::SourceId>>> = Rc::new(Cell::new(None));

        {
            let tick_cell = tick_cell.clone();
            dialog.connect_closed(move |_| {
                if let Some(tick) = tick_cell.take() {
                    tick.remove();
                }
            });
        }

        {
            let backend = backend.clone();
            let runtime = runtime.clone();
            view.connect_load_changed(move |_view, load_event| {
                if load_event == webkit6::LoadEvent::Finished {
                    let backend = backend.clone();
                    bridge::spawn(
                        &runtime,
                        async move { backend.recheck_connectivity().await },
                        |_| {},
                    );
                }
            });
        }

        let tick = glib::timeout_add_seconds_local(5, move || {
            let backend = backend.clone();
            bridge::spawn(
                &runtime,
                async move { backend.recheck_connectivity().await },
                |_| {},
            );
            glib::ControlFlow::Continue
        });
        tick_cell.set(Some(tick));

        match page {
            PortalPage::Uri(uri) => view.load_uri(&uri),
            PortalPage::Builtin => view.load_html(&builtin_portal_html(), None),
        }

        dialog.present(Some(parent));

        Self {
            dialog,
            tick: tick_cell,
        }
    }

    /// Closed both by the step (once connectivity reaches `Full`) and,
    /// harmlessly a second time, by `connect_closed` above if the user
    /// dismissed the window themselves — `Cell::take` makes either order safe.
    pub fn close(&self) {
        if let Some(tick) = self.tick.take() {
            tick.remove();
        }
        self.dialog.force_close();
    }
}

fn builtin_portal_html() -> String {
    format!(
        r#"<!DOCTYPE html>
<html>
<head><meta charset="utf-8"></head>
<body style="font-family: sans-serif; max-width: 32rem; margin: 4rem auto; text-align: center; color: #222;">
<h1>{title}</h1>
<p>{body}</p>
<p><a href="{sentinel}" style="display:inline-block; padding:0.75rem 1.5rem; background:#3584e4; color:#fff; border-radius:6px; text-decoration:none;">{cta}</a></p>
</body>
</html>"#,
        title = glib::markup_escape_text(&tr("Modulix demo captive portal")),
        body = glib::markup_escape_text(&tr(
            "This is an offline demo page simulating a Wi-Fi captive portal login screen. Click the button below to simulate signing in and continue the installation."
        )),
        sentinel = PORTAL_SENTINEL_URI,
        cta = glib::markup_escape_text(&tr("Sign in")),
    )
}
