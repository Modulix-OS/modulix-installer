mod a11y;
mod app;
mod backend;
mod bridge;
mod config;
#[cfg(debug_assertions)]
mod debugsim;
mod engine;
mod finish;
mod i18n;
mod mx;
mod secret;
mod steps;
mod welcome;
mod widgets;

use gtk::prelude::*;

fn main() -> glib::ExitCode {
    i18n::init();

    let args: Vec<String> = std::env::args().skip(1).collect();

    // `--fake-disks=`/`--fake-error=`/`--fake-instant` only exist in a debug
    // build (see `debugsim`'s doc comment on why this is never a fallback).
    // A malformed flag or `--fake-error=list` is resolved here, before GTK or
    // tokio exist: a developer typing a typo at a terminal gets a plain exit,
    // not a GTK window hanging for lack of a display.
    #[cfg(debug_assertions)]
    let fake_cfg = match debugsim::cli::parse(&args) {
        debugsim::cli::CliOutcome::Real => None,
        debugsim::cli::CliOutcome::Fake(cfg) => Some(cfg),
        debugsim::cli::CliOutcome::PrintAndExit(text) => {
            println!("{text}");
            return glib::ExitCode::SUCCESS;
        }
        debugsim::cli::CliOutcome::Error(msg) => {
            eprintln!("modulixos-installer: {msg}");
            return glib::ExitCode::FAILURE;
        }
    };
    // In release, the exact same `--`-prefixed argument is refused instead
    // of silently ignored (`run_with_args` used to drop argv on the floor) —
    // but the live ISO's kiosk session has no terminal, only the fullscreen
    // GTK window, so this one has to go through `app::build_fatal_window`
    // rather than exiting before GTK starts.
    #[cfg(not(debug_assertions))]
    let cli_error: Option<String> = args.iter().find(|a| a.starts_with("--")).map(|bad| {
        format!(
            "{bad}: debug-only flags (--fake-disks, --fake-error, --fake-instant) and \
             unknown flags are refused in a release build"
        )
    });
    #[cfg(debug_assertions)]
    let cli_error: Option<String> = None;

    // libadwaita, the stylesheet and the resource bundle come up before the
    // backends: the fatal path below has to build real GTK widgets, and under
    // the kiosk session there is no terminal to fall back to.
    adw::init().expect("failed to initialize libadwaita");
    // Dark is selected here rather than through `GTK_THEME` in the kiosk
    // module: libadwaita drops its own stylesheet when `GTK_THEME` is set,
    // leaving the app on GTK4's built-in Adwaita.
    adw::StyleManager::default().set_color_scheme(adw::ColorScheme::PreferDark);
    gio::resources_register_include!("modulixos-installer.gresource")
        .expect("failed to load bundled resources");

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("failed to start the tokio runtime");

    #[cfg(debug_assertions)]
    if let Some(cfg) = fake_cfg {
        debugsim::install(debugsim::DebugSim::new(cfg.disks, cfg.fault, cfg.instant));
    }

    // A failed backend connection is fatal, never a fallback. There is no
    // simulated mode to degrade into: an installer that simulates a whole
    // install and then reports success is worse than one that refuses to run.
    // A release build's CLI refusal is funnelled through the exact same
    // fatal path, so it's visible on the kiosk's screen.
    let backends = match cli_error {
        Some(reason) => {
            eprintln!("modulixos-installer: {reason}");
            Err(reason)
        }
        None => match runtime.block_on(async {
            #[cfg(debug_assertions)]
            if let Some(sim) = debugsim::active() {
                return backend::Backends::with_fake_disks(sim.disks.clone(), sim.crypt.clone())
                    .await;
            }
            backend::Backends::new().await
        }) {
            Ok(backends) => Ok(backends),
            Err(e) => {
                let reason = mx::render(&e);
                eprintln!(
                    "modulixos-installer: a required system service could not be reached: {reason}"
                );
                Err(reason)
            }
        },
    };
    let fatal = backends.is_err();

    let application = adw::Application::builder()
        .application_id("org.modulix.Installer")
        .build();

    let runtime_handle = runtime.handle().clone();
    application.connect_activate(move |gtk_app| {
        if let Some(display) = gtk::gdk::Display::default() {
            gtk::IconTheme::for_display(&display).add_resource_path("/org/modulix/installer/icons");
        }
        match &backends {
            Ok(backends) => {
                app::build_window(gtk_app, backends.clone(), runtime_handle.clone());
            }
            Err(reason) => app::build_fatal_window(gtk_app, reason),
        }
    });

    let code = application.run_with_args(&[] as &[&str]);
    if fatal { glib::ExitCode::FAILURE } else { code }
}
