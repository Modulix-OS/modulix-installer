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

    // A failed backend connection is fatal, never a fallback. There is no
    // simulated mode to degrade into: an installer that simulates a whole
    // install and then reports success is worse than one that refuses to run.
    let backends = match runtime.block_on(backend::Backends::new()) {
        Ok(backends) => Ok(backends),
        Err(e) => {
            let reason = mx::render(&e);
            eprintln!(
                "modulixos-installer: a required system service could not be reached: {reason}"
            );
            Err(reason)
        }
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
