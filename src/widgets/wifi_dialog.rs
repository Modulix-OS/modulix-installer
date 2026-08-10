//! Wi-Fi password prompt (network step) — `adw::AlertDialog`, re-shown with an
//! error message on rejection via simple recursion (no self-referencing closure).

use crate::backend::net::{NetworkBackend, WifiConnectRequest, WifiSecurity};
use crate::bridge;
use crate::i18n::tr;
use crate::mx;
use crate::widgets::ApRow;
use adw::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

#[derive(Clone)]
pub struct WifiCredentials {
    pub ssid: String,
    pub password: String,
    pub security: WifiSecurity,
}

/// Deliberately doesn't hold the `NetworkStep` itself — just the pieces
/// `prompt_and_connect` needs — so there's no `Rc` cycle back to the step.
#[derive(Clone)]
pub struct ConnectCtx {
    pub backend: Arc<dyn NetworkBackend>,
    pub runtime: tokio::runtime::Handle,
    pub parent: gtk::Widget,
    pub connected_ssid: Rc<RefCell<Option<String>>>,
    pub ap_rows: Rc<RefCell<Vec<ApRow>>>,
}

fn set_row_busy(ctx: &ConnectCtx, ssid: &str, busy: bool) {
    if let Some(row) = ctx.ap_rows.borrow().iter().find(|r| r.ssid() == ssid) {
        row.set_busy(busy);
    }
}

fn set_row_error(ctx: &ConnectCtx, ssid: &str, message: Option<&str>) {
    if let Some(row) = ctx.ap_rows.borrow().iter().find(|r| r.ssid() == ssid) {
        row.set_error(message);
    }
}

/// Flags `ssid`'s row as the active connection and clears the flag on every
/// other row — called on a successful connect so the checkmark doesn't wait
/// for the next rescan to catch up.
pub(crate) fn mark_connected_row(ctx: &ConnectCtx, ssid: &str) {
    for row in ctx.ap_rows.borrow().iter() {
        row.set_active(row.ssid() == ssid);
    }
}

fn security_string_list() -> gtk::StringList {
    gtk::StringList::new(&[
        &tr("Open (no password)"),
        &tr("WPA/WPA2 Personal"),
        &tr("WPA3 Personal"),
    ])
}

fn security_from_combo(selected: u32) -> WifiSecurity {
    match selected {
        0 => WifiSecurity::Open,
        2 => WifiSecurity::Sae,
        _ => WifiSecurity::Psk,
    }
}

fn combo_index_for_security(security: WifiSecurity) -> u32 {
    match security {
        WifiSecurity::Open => 0,
        WifiSecurity::Sae => 2,
        WifiSecurity::Psk | WifiSecurity::Enterprise => 1,
    }
}

struct DialogRows {
    password: adw::PasswordEntryRow,
    ssid: Option<adw::EntryRow>,
    security: Option<adw::ComboRow>,
}

fn build_dialog(
    prefill: &WifiCredentials,
    hidden: bool,
    error: Option<&str>,
) -> (adw::AlertDialog, DialogRows) {
    let group = adw::PreferencesGroup::new();

    let ssid_row = hidden.then(|| {
        let row = adw::EntryRow::builder()
            .title(tr("Network name (SSID)"))
            .text(prefill.ssid.as_str())
            .build();
        group.add(&row);
        row
    });

    let security_row = hidden.then(|| {
        let row = adw::ComboRow::builder()
            .title(tr("Security"))
            .model(&security_string_list())
            .build();
        row.set_selected(combo_index_for_security(prefill.security));
        group.add(&row);
        row
    });

    let password_row = adw::PasswordEntryRow::builder()
        .title(tr("Password"))
        .text(prefill.password.as_str())
        .build();
    group.add(&password_row);

    if let Some(message) = error {
        let error_label = gtk::Label::builder()
            .label(message)
            .wrap(true)
            .xalign(0.0)
            .build();
        error_label.add_css_class("error");
        error_label.add_css_class("caption");
        group.add(&error_label);
    }

    let heading = if hidden {
        tr("Connect to a hidden network")
    } else {
        prefill.ssid.clone()
    };
    let dialog = adw::AlertDialog::builder()
        .heading(heading)
        .body(tr("Enter the password for this Wi-Fi network"))
        .extra_child(&group)
        .build();
    dialog.add_response("cancel", &tr("Cancel"));
    dialog.add_response("connect", &tr("Connect"));
    dialog.set_response_appearance("connect", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("connect"));
    dialog.set_close_response("cancel");

    (
        dialog,
        DialogRows {
            password: password_row,
            ssid: ssid_row,
            security: security_row,
        },
    )
}

/// Shows the password dialog, attempts the connection on "Connect", and on
/// rejection re-shows itself with the error message — a plain recursive
/// function rather than a closure that needs to reference itself.
pub fn prompt_and_connect(
    ctx: ConnectCtx,
    prefill: WifiCredentials,
    hidden: bool,
    error: Option<String>,
) {
    let (dialog, rows) = build_dialog(&prefill, hidden, error.as_deref());
    let parent = ctx.parent.clone();
    glib::spawn_future_local(async move {
        let response = dialog.choose_future(Some(&parent)).await;
        if response != "connect" {
            return;
        }

        let ssid = rows
            .ssid
            .as_ref()
            .map(|r| r.text().to_string())
            .unwrap_or_else(|| prefill.ssid.clone());
        let security = rows
            .security
            .as_ref()
            .map(|r| security_from_combo(r.selected()))
            .unwrap_or(prefill.security);
        let password_text = rows.password.text().to_string();
        let password = (security != WifiSecurity::Open).then_some(password_text.clone());

        set_row_busy(&ctx, &ssid, true);

        let request = WifiConnectRequest {
            ssid: ssid.clone(),
            password,
            security,
            hidden,
        };
        let backend = ctx.backend.clone();
        let ctx_for_result = ctx.clone();
        let ssid_for_result = ssid.clone();
        let creds = WifiCredentials {
            ssid: ssid.clone(),
            password: password_text,
            security,
        };
        bridge::spawn(
            &ctx.runtime,
            async move { backend.connect_wifi(request).await },
            move |result| {
                set_row_busy(&ctx_for_result, &ssid_for_result, false);
                match result {
                    Ok(()) => {
                        *ctx_for_result.connected_ssid.borrow_mut() = Some(ssid_for_result.clone());
                        set_row_error(&ctx_for_result, &ssid_for_result, None);
                        mark_connected_row(&ctx_for_result, &ssid_for_result);
                    }
                    Err(e) => {
                        let message = match &e {
                            mx::Error::Backend(msgid) => tr(msgid),
                            other => other.to_string(),
                        };
                        set_row_error(&ctx_for_result, &ssid_for_result, Some(&message));
                        prompt_and_connect(ctx_for_result, creds, hidden, Some(message));
                    }
                }
            },
        );
    });
}
