use crate::backend::Backends;
use crate::backend::net::Connectivity;
use crate::bridge;
use crate::config::InstallConfig;
use crate::i18n::tr;
use crate::mx;
use crate::steps::{Step, StepId, ValidityTracker};
use crate::widgets::ApRow;
use adw::prelude::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

/// Step 6 — stub for iteration 1: lists Wi-Fi networks and shows
/// connectivity, but the captive-portal WebKitGTK window (see CLAUDE.md) is
/// iteration 2.
pub struct NetworkStep {
    widget: gtk::Widget,
    group: adw::PreferencesGroup,
    status_row: adw::ActionRow,
    ap_list: adw::PreferencesGroup,
    connected: Rc<Cell<bool>>,
    behind_portal: Rc<Cell<bool>>,
    last_connectivity: Rc<Cell<Connectivity>>,
    connected_ssid: Rc<RefCell<Option<String>>>,
    validity: ValidityTracker,
}

impl NetworkStep {
    pub fn new(backends: &Backends, runtime: &tokio::runtime::Handle) -> Self {
        let connected = Rc::new(Cell::new(false));
        let behind_portal = Rc::new(Cell::new(false));
        let last_connectivity = Rc::new(Cell::new(Connectivity::Unknown));
        let connected_ssid: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));

        let status_row = adw::ActionRow::builder()
            .title(tr("Status"))
            .subtitle(tr("Checking…"))
            .build();
        let group = adw::PreferencesGroup::builder()
            .title(tr("Network"))
            .build();
        group.add(&status_row);

        let ap_list = adw::PreferencesGroup::builder()
            .title(tr("Wi-Fi networks"))
            .build();

        let page = adw::PreferencesPage::new();
        page.add(&group);
        page.add(&ap_list);

        {
            let network = backends.network.clone();
            let status_row = status_row.clone();
            let connected = connected.clone();
            let behind_portal = behind_portal.clone();
            let last_connectivity = last_connectivity.clone();
            bridge::spawn(
                runtime,
                async move { network.connectivity().await },
                move |result| {
                    let Ok(connectivity) = result else { return };
                    connected.set(matches!(
                        connectivity,
                        Connectivity::Full | Connectivity::Limited
                    ));
                    behind_portal.set(connectivity == Connectivity::Portal);
                    last_connectivity.set(connectivity);
                    status_row.set_subtitle(&tr(connectivity_label(connectivity)));
                },
            );
        }

        {
            let network_backend = backends.network.clone();
            let network_backend_for_scan = network_backend.clone();
            let runtime_handle = runtime.clone();
            let ap_list = ap_list.clone();
            let connected_ssid = connected_ssid.clone();
            bridge::spawn(
                runtime,
                async move { network_backend_for_scan.scan_wifi().await },
                move |result| {
                    let Ok(access_points) = result else { return };
                    for ap in access_points {
                        // Iteration-1 stub: only open networks can be joined from
                        // here — secured networks need a password prompt, which
                        // is out of scope until the step is made fully functional.
                        let ap_row = ApRow::new(&ap);
                        if !ap.secured {
                            let network_backend = network_backend.clone();
                            let runtime_handle = runtime_handle.clone();
                            let ssid = ap.ssid.clone();
                            let connected_ssid = connected_ssid.clone();
                            ap_row.connect_activated(move |_row| {
                                let network_backend = network_backend.clone();
                                let ssid_for_call = ssid.clone();
                                let ssid_for_result = ssid.clone();
                                let connected_ssid = connected_ssid.clone();
                                bridge::spawn(
                                    &runtime_handle,
                                    async move {
                                        network_backend.connect_wifi(&ssid_for_call, None).await
                                    },
                                    move |result| match result {
                                        Ok(()) => {
                                            *connected_ssid.borrow_mut() = Some(ssid_for_result)
                                        }
                                        Err(e) => {
                                            eprintln!("failed to connect to Wi-Fi network: {e}")
                                        }
                                    },
                                );
                            });
                        }
                        ap_list.add(&ap_row.widget());
                    }
                },
            );
        }

        Self {
            widget: page.upcast(),
            group,
            status_row,
            ap_list,
            connected,
            behind_portal,
            last_connectivity,
            connected_ssid,
            validity: ValidityTracker::ready(),
        }
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
        cfg.network.connected = self.connected.get();
        cfg.network.behind_captive_portal = self.behind_portal.get();
        cfg.network.connection_name = self.connected_ssid.borrow().clone();
        Ok(())
    }

    fn retranslate(&self) {
        self.group.set_title(&tr("Network"));
        self.status_row.set_title(&tr("Status"));
        self.status_row
            .set_subtitle(&tr(connectivity_label(self.last_connectivity.get())));
        self.ap_list.set_title(&tr("Wi-Fi networks"));
    }
}
