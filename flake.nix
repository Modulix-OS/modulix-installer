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

              # runtime tools the partitioning step 7 pipeline shells out to
              # (backend/disk/udisks2.rs, engine/tasks/*):
              # these must become *runtime* dependencies of the ISO module
              # (mxpkgs/installer/default.nix) once it switches off Calamares,
              # same as webkitgtk_6_0 above.
              ntfs3g # ntfsresize, AlongsideWindows shrink preflight
              gptfdisk # udisks2's GPT partition-table backend
              util-linux # mkswap/swapon, mount/umount
              cryptsetup # LUKS2 format/open
              tpm2-tools # systemd-cryptenroll --tpm2-device
              udisks2 # org.freedesktop.UDisks2 D-Bus service
              gparted # step 7 manual mode's partition editor — creating/resizing/
                      # deleting partitions is delegated to it entirely
              gnome-calculator # step 7's header-bar calculator button (app.rs)

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
            # So `setlocale(LC_ALL, "fr_FR.UTF-8")` (formatting only — message
            # switching goes through `LANGUAGE`, see i18n.rs) can find a
            # generated locale in a plain `nix develop` shell. Both variables
            # are required: nixpkgs' glibc prefers the version-suffixed
            # `LOCALE_ARCHIVE_2_27` over `LOCALE_ARCHIVE` when present, and an
            # ambient `LOCALE_ARCHIVE_2_27` from the host profile (a slim,
            # 5-locale archive) silently shadows the 842-locale one below.
            LOCALE_ARCHIVE = "${pkgs.glibcLocales}/lib/locale/locale-archive";
            LOCALE_ARCHIVE_2_27 = "${pkgs.glibcLocales}/lib/locale/locale-archive";
          };
        });
    };
}
