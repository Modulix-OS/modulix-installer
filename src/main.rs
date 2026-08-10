mod a11y;
mod app;
mod backend;
mod bridge;
mod config;
mod engine;
mod finish;
mod i18n;
mod mx;
mod steps;
mod widgets;

use gtk::prelude::*;

fn main() -> glib::ExitCode {
    i18n::init();

    let args: Vec<String> = std::env::args().collect();
    let fake_disk_arg = args.iter().find_map(|a| a.strip_prefix("--fake-disk="));
    let fake = args.iter().any(|a| a == "--fake") || fake_disk_arg.is_some();
    let windowed = args.iter().any(|a| a == "--windowed");

    let fake_disk_scenario = match fake_disk_arg {
        Some(name) => match backend::disk::DiskScenario::parse(name) {
            Some(scenario) => scenario,
            None => {
                let valid: Vec<&str> = backend::disk::DiskScenario::ALL
                    .iter()
                    .map(|s| s.name())
                    .collect();
                eprintln!(
                    "unknown --fake-disk scenario {name:?}, expected one of: {}",
                    valid.join(", ")
                );
                std::process::exit(1);
            }
        },
        None => backend::disk::DiskScenario::Linux,
    };

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("failed to start the tokio runtime");

    let backends = if fake {
        backend::Backends::fake_with(fake_disk_scenario)
    } else {
        match runtime.block_on(backend::Backends::real()) {
            Ok(backends) => backends,
            Err(e) => {
                eprintln!("failed to connect real backends, falling back to --fake: {e}");
                backend::Backends::fake()
            }
        }
    };

    adw::init().expect("failed to initialize libadwaita");
    gio::resources_register_include!("modulixos-installer.gresource")
        .expect("failed to load bundled resources");

    let application = adw::Application::builder()
        .application_id("org.modulix.Installer")
        .build();

    let runtime_handle = runtime.handle().clone();
    application.connect_activate(move |gtk_app| {
        if let Some(display) = gtk::gdk::Display::default() {
            gtk::IconTheme::for_display(&display).add_resource_path("/org/modulix/installer/icons");
        }
        app::build_window(gtk_app, backends.clone(), runtime_handle.clone(), windowed);
    });

    application.run_with_args(&[] as &[&str])
}
