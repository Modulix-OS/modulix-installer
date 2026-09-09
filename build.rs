use std::env;
use std::fs;
use std::path::Path;
use std::process::Command;

fn main() {
    // App-pack card icons (see data/resources.gresource.xml) resolve against
    // this extra sourcedir instead of a vendored copy — set by flake.nix's
    // devShell to `${pkgs.papirus-icon-theme}/share/icons/Papirus/48x48`.
    let papirus_48 = env::var("PAPIRUS_ICON_THEME_48")
        .expect("PAPIRUS_ICON_THEME_48 must be set (see flake.nix devShell)");
    println!("cargo:rerun-if-env-changed=PAPIRUS_ICON_THEME_48");

    glib_build_tools::compile_resources(
        &["data", &papirus_48],
        "data/resources.gresource.xml",
        "modulixos-installer.gresource",
    );
    compile_translations();
}

/// Compiles every `po/<lang>.po` into `$OUT_DIR/locale/<lang>/LC_MESSAGES/modulixos-installer.mo`
/// and points debug builds at that directory (see `src/i18n.rs`) — release
/// builds bind to the system locale dir instead, same debug/release split as
/// `modulix-core-utils`' `CONFIG_DIRECTORY`.
fn compile_translations() {
    println!("cargo:rerun-if-changed=po");

    let out_dir = env::var("OUT_DIR").expect("OUT_DIR set by cargo");
    let locale_dir = Path::new(&out_dir).join("locale");
    // Emitted unconditionally: i18n.rs's `env!("MODULIX_LOCALE_DIR")` needs
    // this to exist even if po/ is empty or missing.
    println!(
        "cargo:rustc-env=MODULIX_LOCALE_DIR={}",
        locale_dir.display()
    );

    let po_dir = Path::new("po");
    if !po_dir.exists() {
        return;
    }

    for entry in fs::read_dir(po_dir).expect("read po/ directory") {
        let entry = entry.expect("read po/ directory entry");
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("po") {
            continue;
        }
        let lang = path
            .file_stem()
            .and_then(|s| s.to_str())
            .expect("po file has a stem");

        let dest_dir = locale_dir.join(lang).join("LC_MESSAGES");
        fs::create_dir_all(&dest_dir).expect("create locale output dir");
        let dest = dest_dir.join("modulixos-installer.mo");

        let status = Command::new("msgfmt")
            .arg(&path)
            .arg("-o")
            .arg(&dest)
            .status()
            .expect("run msgfmt (gettext must be in PATH — see flake.nix devShell)");
        assert!(status.success(), "msgfmt failed for {}", path.display());
    }
}
