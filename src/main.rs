mod app;
// `backend`'s trait catalogue is deliberately broader than what the wizard's
// UI exercises today (e.g. `CryptBackend`, most of `DiskBackend`), and
// `engine` (the install `Task` pipeline) isn't wired to any UI action yet —
// both are iteration-1 scaffolding for `init_all`/real installs in
// iteration 2 (see CLAUDE.md). Suppress dead_code for that reason rather
// than scattering per-item allows.
#[allow(dead_code)]
mod backend;
mod bridge;
mod config;
#[allow(dead_code)]
mod engine;
mod i18n;
mod mx;
mod steps;
mod widgets;

use gtk::prelude::*;

fn main() -> glib::ExitCode {
    i18n::init();

    let args: Vec<String> = std::env::args().collect();
    let fake = args.iter().any(|a| a == "--fake");
    let windowed = args.iter().any(|a| a == "--windowed");

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("failed to start the tokio runtime");

    let backends = if fake {
        backend::Backends::fake()
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
