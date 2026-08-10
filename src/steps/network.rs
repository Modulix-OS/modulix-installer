use crate::backend::Backends;
use crate::backend::net::{Connectivity, PortalPage, WifiConnectRequest, WifiSecurity};
use crate::bridge;
use crate::config::InstallConfig;
use crate::i18n::tr;
use crate::mx;
use crate::steps::{Step, StepId, ValidityTracker};
use crate::widgets::wifi_dialog::mark_connected_row;
use crate::widgets::{ApRow, ConnectCtx, PortalWindow, WifiCredentials, prompt_and_connect};
use adw::prelude::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

/// Step 6 — real NetworkManager connectivity: ethernet, Wi-Fi (open/secured/
/// hidden), captive-portal sign-in via an embedded WebKitGTK window, and a
/// gate that blocks "Next" until connectivity looks usable (`nixos-install`
/// fetches from `cache.nixos.org` — leaving without a network just delays the
/// failure by 20 minutes).
pub struct NetworkStep {
    widget: gtk::Widget,
    group: adw::PreferencesGroup,
    status_row: adw::ActionRow,
    ethernet_row: adw::ActionRow,
    ap_group: adw::PreferencesGroup,
    rescan_button: gtk::Button,
    other_group: adw::PreferencesGroup,
    hidden_row: adw::ActionRow,
    ap_list_state: Rc<Cell<ApListState>>,
    last_connectivity: Rc<Cell<Connectivity>>,
    override_accepted: Rc<Cell<bool>>,
    connected_ssid: Rc<RefCell<Option<String>>>,
    /// Re-invoked by `retranslate()` so the banner/status text picks up the
    /// new language without re-deriving the whole gate decision by hand.
    apply_connectivity: Rc<dyn Fn(Connectivity)>,
    validity: ValidityTracker,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ApListState {
    Loading,
    Empty,
    Populated,
}

pub(crate) enum Gate {
    Ready,
    Blocked {
        reason: &'static str,
        offer_override: bool,
    },
}

/// `Limited`/`Unknown` can be overridden ("Continue anyway"); `Portal`/`None`
/// are hard blocks — signing in or connecting is the only way past them.
pub(crate) fn gate_for(connectivity: Connectivity, override_accepted: bool) -> Gate {
    match connectivity {
        Connectivity::Full => Gate::Ready,
        Connectivity::Limited if override_accepted => Gate::Ready,
        Connectivity::Limited => Gate::Blocked {
            reason: "No Internet access detected — the installation may fail",
            offer_override: true,
        },
        Connectivity::Unknown if override_accepted => Gate::Ready,
        Connectivity::Unknown => Gate::Blocked {
            reason: "Couldn't determine whether this machine has Internet access",
            offer_override: true,
        },
        Connectivity::Portal => Gate::Blocked {
            reason: "Sign in to the Wi-Fi portal to continue",
            offer_override: false,
        },
        Connectivity::None => Gate::Blocked {
            reason: "Connect to a network to continue",
            offer_override: false,
        },
    }
}

fn connectivity_label(connectivity: Connectivity) -> &'static str {
    match connectivity {
        Connectivity::Full => "Connected",
        Connectivity::Limited => "Limited connectivity",
        Connectivity::Portal => "Behind a captive portal",
        Connectivity::None => "Not connected",
        Connectivity::Unknown => "Unknown",
    }
}

fn render_backend_error(e: &mx::Error) -> String {
    match e {
        mx::Error::Backend(msgid) => tr(msgid),
        other => other.to_string(),
    }
}

/// Wires the click handler for one scanned access point row. Enterprise rows
/// are left untouched — `ApRow::new` already made them non-activatable.
/// Open networks connect directly; anything else opens the password dialog.
fn wire_ap_row_click(ap_row: &ApRow, ssid: &str, security: WifiSecurity, ctx: &ConnectCtx) {
    match security {
        WifiSecurity::Enterprise => {}
        WifiSecurity::Open => {
            let ssid = ssid.to_string();
            let ctx = ctx.clone();
            let row_for_closure = ap_row.clone();
            ap_row.connect_activated(move |_row| {
                let ap_row = row_for_closure.clone();
                ap_row.set_busy(true);
                ap_row.set_error(None);
                let ssid_for_call = ssid.clone();
                let ssid_for_result = ssid.clone();
                let backend = ctx.backend.clone();
                let connected_ssid = ctx.connected_ssid.clone();
                let ctx_for_result = ctx.clone();
                bridge::spawn(
                    &ctx.runtime,
                    async move {
                        backend
                            .connect_wifi(WifiConnectRequest {
                                ssid: ssid_for_call,
                                password: None,
                                security: WifiSecurity::Open,
                                hidden: false,
                            })
                            .await
                    },
                    move |result| {
                        ap_row.set_busy(false);
                        match result {
                            Ok(()) => {
                                *connected_ssid.borrow_mut() = Some(ssid_for_result.clone());
                                mark_connected_row(&ctx_for_result, &ssid_for_result);
                            }
                            Err(e) => ap_row.set_error(Some(&render_backend_error(&e))),
                        }
                    },
                );
            });
        }
        WifiSecurity::Psk | WifiSecurity::Sae => {
            let ctx = ctx.clone();
            let ssid = ssid.to_string();
            ap_row.connect_activated(move |_row| {
                prompt_and_connect(
                    ctx.clone(),
                    WifiCredentials {
                        ssid: ssid.clone(),
                        password: String::new(),
                        security,
                    },
                    false,
                    None,
                );
            });
        }
    }
}

impl NetworkStep {
    pub fn new(backends: &Backends, runtime: &tokio::runtime::Handle) -> Self {
        let ap_rows: Rc<RefCell<Vec<ApRow>>> = Rc::new(RefCell::new(Vec::new()));
        let ap_list_state = Rc::new(Cell::new(ApListState::Loading));
        let last_connectivity = Rc::new(Cell::new(Connectivity::default()));
        let override_accepted = Rc::new(Cell::new(false));
        let connected_ssid: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
        let portal_window: Rc<RefCell<Option<PortalWindow>>> = Rc::new(RefCell::new(None));

        let root = adw::ToolbarView::new();
        let widget: gtk::Widget = root.clone().upcast();

        let ctx = ConnectCtx {
            backend: backends.network.clone(),
            runtime: runtime.clone(),
            parent: widget.clone(),
            connected_ssid: connected_ssid.clone(),
            ap_rows: ap_rows.clone(),
        };

        let banner = adw::Banner::new("");

        let status_row = adw::ActionRow::builder()
            .title(tr("Status"))
            .subtitle(tr("Checking…"))
            .build();
        let ethernet_row = adw::ActionRow::builder()
            .title(tr("Wired connection"))
            .subtitle(tr("Not connected"))
            .build();
        let group = adw::PreferencesGroup::builder()
            .title(tr("Network"))
            .build();
        group.add(&status_row);
        group.add(&ethernet_row);

        let rescan_button = gtk::Button::from_icon_name("view-refresh-symbolic");
        rescan_button.set_tooltip_text(Some(&tr("Rescan")));
        rescan_button.add_css_class("flat");
        let ap_group = adw::PreferencesGroup::builder()
            .title(tr("Wi-Fi networks"))
            .build();
        ap_group.set_header_suffix(Some(&rescan_button));

        let hidden_row = adw::ActionRow::builder()
            .title(tr("Connect to a hidden network"))
            .activatable(true)
            .build();
        let other_group = adw::PreferencesGroup::builder()
            .title(tr("Other networks"))
            .build();
        other_group.add(&hidden_row);

        {
            let ctx = ctx.clone();
            hidden_row.connect_activated(move |_row| {
                prompt_and_connect(
                    ctx.clone(),
                    WifiCredentials {
                        ssid: String::new(),
                        password: String::new(),
                        security: WifiSecurity::Psk,
                    },
                    true,
                    None,
                );
            });
        }

        let do_scan: Rc<dyn Fn()> = {
            let ap_group = ap_group.clone();
            let ap_rows = ap_rows.clone();
            let ap_list_state = ap_list_state.clone();
            let rescan_button = rescan_button.clone();
            let network_backend = backends.network.clone();
            let runtime = runtime.clone();
            let ctx = ctx.clone();
            Rc::new(move || {
                rescan_button.set_sensitive(false);
                ap_list_state.set(ApListState::Loading);
                ap_group.set_description(Some(&tr("Scanning…")));
                for row in ap_rows.borrow_mut().drain(..) {
                    ap_group.remove(&row.widget());
                }

                let ap_group = ap_group.clone();
                let ap_rows = ap_rows.clone();
                let ap_list_state = ap_list_state.clone();
                let rescan_button = rescan_button.clone();
                let ctx = ctx.clone();
                bridge::spawn(
                    &runtime,
                    {
                        let network_backend = network_backend.clone();
                        async move { network_backend.scan_wifi().await }
                    },
                    move |result| {
                        rescan_button.set_sensitive(true);
                        let access_points = result.unwrap_or_default();
                        if access_points.is_empty() {
                            ap_list_state.set(ApListState::Empty);
                            ap_group.set_description(Some(&tr("No Wi-Fi networks found")));
                        } else {
                            ap_list_state.set(ApListState::Populated);
                            ap_group.set_description(None);
                        }
                        for ap in access_points {
                            let ap_row = ApRow::new(&ap);
                            wire_ap_row_click(&ap_row, &ap.ssid, ap.security, &ctx);
                            ap_group.add(&ap_row.widget());
                            ap_rows.borrow_mut().push(ap_row);
                        }
                    },
                );
            })
        };

        {
            let do_scan = do_scan.clone();
            rescan_button.connect_clicked(move |_| do_scan());
        }
        do_scan();

        let page = adw::PreferencesPage::new();
        page.add(&group);
        page.add(&ap_group);
        page.add(&other_group);
        root.add_top_bar(&banner);
        root.set_content(Some(&page));

        let validity = ValidityTracker::blocked(tr("Connect to a network to continue"));

        let apply_connectivity: Rc<dyn Fn(Connectivity)> = {
            let status_row = status_row.clone();
            let ethernet_row = ethernet_row.clone();
            let banner = banner.clone();
            let validity = validity.clone();
            let last_connectivity = last_connectivity.clone();
            let override_accepted = override_accepted.clone();
            let network_backend = backends.network.clone();
            let runtime = runtime.clone();
            let portal_window = portal_window.clone();
            let widget_for_portal = widget.clone();
            Rc::new(move |c: Connectivity| {
                last_connectivity.set(c);
                status_row.set_subtitle(&tr(connectivity_label(c)));

                match gate_for(c, override_accepted.get()) {
                    Gate::Ready => {
                        validity.set_ready();
                        banner.set_revealed(false);
                    }
                    Gate::Blocked {
                        reason,
                        offer_override,
                    } => {
                        let reason_text = tr(reason);
                        validity.set_blocked(reason_text.clone());
                        banner.set_title(&reason_text);
                        banner.set_button_label(
                            offer_override.then(|| tr("Continue anyway")).as_deref(),
                        );
                        banner.set_revealed(true);
                    }
                }

                if c == Connectivity::Portal {
                    if portal_window.borrow().is_none() {
                        let backend_for_fetch = network_backend.clone();
                        let backend_for_present = network_backend.clone();
                        let runtime_for_present = runtime.clone();
                        let widget_for_portal = widget_for_portal.clone();
                        let portal_window_slot = portal_window.clone();
                        bridge::spawn(
                            &runtime,
                            async move { backend_for_fetch.portal_page().await },
                            move |result| {
                                let page = result.unwrap_or(PortalPage::Builtin);
                                if portal_window_slot.borrow().is_none() {
                                    let window = PortalWindow::present(
                                        &widget_for_portal,
                                        backend_for_present,
                                        runtime_for_present,
                                        page,
                                    );
                                    *portal_window_slot.borrow_mut() = Some(window);
                                }
                            },
                        );
                    }
                } else if let Some(window) = portal_window.borrow_mut().take() {
                    window.close();
                }

                {
                    let ethernet_row = ethernet_row.clone();
                    let network_backend = network_backend.clone();
                    bridge::spawn(
                        &runtime,
                        async move { network_backend.is_ethernet_connected().await },
                        move |result| {
                            let connected = result.unwrap_or(false);
                            ethernet_row.set_subtitle(&tr(if connected {
                                "Connected"
                            } else {
                                "Not connected"
                            }));
                        },
                    );
                }
            })
        };

        {
            let override_accepted = override_accepted.clone();
            let apply_connectivity = apply_connectivity.clone();
            let last_connectivity = last_connectivity.clone();
            banner.connect_button_clicked(move |_banner| {
                override_accepted.set(true);
                (apply_connectivity)(last_connectivity.get());
            });
        }

        bridge::spawn_stream(
            runtime,
            {
                let network = backends.network.clone();
                async move { network.watch_connectivity().await }
            },
            {
                let apply_connectivity = apply_connectivity.clone();
                move |c| (apply_connectivity)(c)
            },
        );

        Self {
            widget,
            group,
            status_row,
            ethernet_row,
            ap_group,
            rescan_button,
            other_group,
            hidden_row,
            ap_list_state,
            last_connectivity,
            override_accepted,
            connected_ssid,
            apply_connectivity,
            validity,
        }
    }
}

impl Step for NetworkStep {
    fn id(&self) -> StepId {
        StepId::Network
    }

    fn title(&self) -> String {
        tr("Network")
    }

    fn icon_name(&self) -> &'static str {
        "network-symbolic"
    }

    fn widget(&self) -> gtk::Widget {
        self.widget.clone()
    }

    fn validity(&self) -> ValidityTracker {
        self.validity.clone()
    }

    fn commit(&self, cfg: &mut InstallConfig) -> mx::Result<()> {
        let c = self.last_connectivity.get();
        cfg.network.connected = matches!(c, Connectivity::Full | Connectivity::Limited);
        cfg.network.behind_captive_portal = c == Connectivity::Portal;
        cfg.network.connection_name = self.connected_ssid.borrow().clone();
        cfg.network.proceeded_without_internet =
            self.override_accepted.get() && c != Connectivity::Full;
        Ok(())
    }

    fn retranslate(&self) {
        self.group.set_title(&tr("Network"));
        self.status_row.set_title(&tr("Status"));
        self.ethernet_row.set_title(&tr("Wired connection"));
        self.ap_group.set_title(&tr("Wi-Fi networks"));
        match self.ap_list_state.get() {
            ApListState::Loading => self.ap_group.set_description(Some(&tr("Scanning…"))),
            ApListState::Empty => self
                .ap_group
                .set_description(Some(&tr("No Wi-Fi networks found"))),
            ApListState::Populated => self.ap_group.set_description(None),
        }
        self.rescan_button.set_tooltip_text(Some(&tr("Rescan")));
        self.other_group.set_title(&tr("Other networks"));
        self.hidden_row
            .set_title(&tr("Connect to a hidden network"));
        (self.apply_connectivity)(self.last_connectivity.get());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_is_always_ready() {
        assert!(matches!(gate_for(Connectivity::Full, false), Gate::Ready));
        assert!(matches!(gate_for(Connectivity::Full, true), Gate::Ready));
    }

    #[test]
    fn limited_blocks_without_override_and_allows_with() {
        assert!(matches!(
            gate_for(Connectivity::Limited, false),
            Gate::Blocked {
                offer_override: true,
                ..
            }
        ));
        assert!(matches!(gate_for(Connectivity::Limited, true), Gate::Ready));
    }

    #[test]
    fn unknown_blocks_without_override_and_allows_with() {
        assert!(matches!(
            gate_for(Connectivity::Unknown, false),
            Gate::Blocked {
                offer_override: true,
                ..
            }
        ));
        assert!(matches!(gate_for(Connectivity::Unknown, true), Gate::Ready));
    }

    #[test]
    fn portal_always_blocks_regardless_of_override() {
        assert!(matches!(
            gate_for(Connectivity::Portal, false),
            Gate::Blocked {
                offer_override: false,
                ..
            }
        ));
        assert!(matches!(
            gate_for(Connectivity::Portal, true),
            Gate::Blocked {
                offer_override: false,
                ..
            }
        ));
    }

    #[test]
    fn none_always_blocks_regardless_of_override() {
        assert!(matches!(
            gate_for(Connectivity::None, false),
            Gate::Blocked {
                offer_override: false,
                ..
            }
        ));
        assert!(matches!(
            gate_for(Connectivity::None, true),
            Gate::Blocked {
                offer_override: false,
                ..
            }
        ));
    }

    #[test]
    fn connectivity_labels_are_stable_msgids() {
        assert_eq!(connectivity_label(Connectivity::Full), "Connected");
        assert_eq!(
            connectivity_label(Connectivity::Limited),
            "Limited connectivity"
        );
        assert_eq!(
            connectivity_label(Connectivity::Portal),
            "Behind a captive portal"
        );
        assert_eq!(connectivity_label(Connectivity::None), "Not connected");
        assert_eq!(connectivity_label(Connectivity::Unknown), "Unknown");
    }
}
