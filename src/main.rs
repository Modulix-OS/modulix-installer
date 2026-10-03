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
    let windowed = args.iter().any(|a| a == "--windowed");

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("failed to start the tokio runtime");

    // A failed backend connection is fatal, never a fallback. There is no
    // simulated mode to degrade into: an installer that simulates a whole
    // install and then reports success is worse than one that refuses to run.
    let backends = match runtime.block_on(backend::Backends::new()) {
        Ok(backends) => backends,
        Err(e) => {
            eprintln!("modulixos-installer: a required system service could not be reached: {e}");
            std::process::exit(1);
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
