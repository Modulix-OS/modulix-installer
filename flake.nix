{
  description = "modulixos-installer — fullscreen GTK4/libadwaita installer for Modulix OS";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  };

  outputs = { self, nixpkgs }:
    let
      supportedSystems = [ "x86_64-linux" "aarch64-linux" ];
      forAllSystems = f: builtins.listToAttrs (map (system: {
        name = system;
        value = f system;
      }) supportedSystems);
    in {
      devShells = forAllSystems (system:
        let
          pkgs = import nixpkgs { inherit system; };
        in {
          default = pkgs.mkShell {
            nativeBuildInputs = with pkgs; [
              pkg-config
              glib # glib-compile-resources, used by build.rs via glib-build-tools
              gettext # msgfmt/xgettext for i18n
            ];
            buildInputs = with pkgs; [
              cargo
              rustc
              rustfmt
              clippy
              rust-analyzer

              glib
              glib.dev
              gtk4
              gtk4.dev
              libadwaita
              libadwaita.dev
              webkitgtk_6_0
              webkitgtk_6_0.dev
              openssl

              # runtime tools modulix-core-utils' `detect-hardware` feature shells out to
              pciutils
              usbutils
              cpuid

              # locale/timezone/keyboard catalogs (src/backend/locale/catalog.rs) —
              # on the real NixOS ISO these are reachable at plain FHS-ish paths;
              # a bare `nix develop` shell has no such profile, so shellHook below
              # points the catalog reader at these packages directly.
              tzdata
              glibcLocales
              xkeyboard_config
            ];
            RUST_SRC_PATH = "${pkgs.rust.packages.stable.rustPlatform.rustLibSrc}";
            TZDIR = "${pkgs.tzdata}/share/zoneinfo";
            MODULIX_DEV_EVDEV_XML = "${pkgs.xkeyboard_config}/share/X11/xkb/rules/evdev.xml";
            MODULIX_DEV_LOCALE_SUPPORTED = "${pkgs.glibcLocales}/share/i18n/SUPPORTED";
            # So `setlocale(LC_ALL, "fr_FR.UTF-8")` (language step) actually
            # succeeds in a plain `nix develop` shell — without this, glibc
            # has no generated locales to switch to and silently stays on C.
            LOCALE_ARCHIVE = "${pkgs.glibcLocales}/lib/locale/locale-archive";
          };
        });
    };
}
