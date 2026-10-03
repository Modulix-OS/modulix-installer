{
  description = "modulixos-installer — fullscreen GTK4/libadwaita installer for Modulix OS";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    naersk.url = "github:nix-community/naersk";
    naersk.inputs.nixpkgs.follows = "nixpkgs";
    modulix-core-utils = {
      url = "github:Modulix-OS/modulix-core-utils";
      flake = false;
    };
    # Branding only (modulixos/branding.nix + assets/). Deliberately not
    # `follows`-ing nixpkgs: no mxpkgs package output is ever evaluated here,
    # so there is no closure to keep in sync.
    mxpkgs.url = "github:Modulix-OS/mxpkgs";
  };

  outputs = { self, nixpkgs, naersk, modulix-core-utils, mxpkgs }:
    let
      supportedSystems = [ "x86_64-linux" "aarch64-linux" ];
      forAllSystems = f: builtins.listToAttrs (map (system: {
        name = system;
        value = f system;
      }) supportedSystems);

      pkgsFor = system: import nixpkgs { inherit system; };
    in {
      devShells = forAllSystems (system:
        let
          pkgs = pkgsFor system;
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

              # runtime tools the partitioning step 6 pipeline shells out to
              # (backend/disk/udisks2.rs, engine/tasks/*). The ISO carries them
              # through nix/kiosk-module.nix, same as webkitgtk_6_0 above.
              ntfs3g # ntfsresize, AlongsideWindows shrink preflight
              gptfdisk # udisks2's GPT partition-table backend
              util-linux # mkswap/swapon, mount/umount
              cryptsetup # LUKS2 format/open
              tpm2-tools # systemd-cryptenroll --tpm2-device
              udisks2 # org.freedesktop.UDisks2 D-Bus service
              gparted # step 7 manual mode's partition editor — creating/resizing/
                      # deleting partitions is delegated to it entirely
              gnome-calculator # step 7's header-bar calculator button (app.rs)

              papirus-icon-theme # step 9's app-pack card icons — build.rs points
                                  # glib-compile-resources straight at this
                                  # package, see PAPIRUS_ICON_THEME_48 below

              # locale/timezone/keyboard catalogs (src/backend/locale/catalog.rs) —
              # on the real NixOS ISO these are reachable at plain FHS-ish paths;
              # a bare `nix develop` shell has no such profile, so shellHook below
              # points the catalog reader at these packages directly.
              tzdata
              glibcLocales
              xkeyboard_config

              # missing from PATH in a bare devShell too — needed by
              # backend/locale (dumpe2fs), backend/locale/system.rs (swaymsg,
              # a no-op outside the kiosk session) and backend/a11y (orca +
              # speechd)
              e2fsprogs
              sway
              orca
              speechd
            ];
            RUST_SRC_PATH = "${pkgs.rust.packages.stable.rustPlatform.rustLibSrc}";
            TZDIR = "${pkgs.tzdata}/share/zoneinfo";
            PAPIRUS_ICON_THEME_48 = "${pkgs.papirus-icon-theme}/share/icons/Papirus/48x48";
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

      packages = forAllSystems (system:
        let
          pkgs = pkgsFor system;
          naerskLib = pkgs.callPackage naersk { };
          modulixos-installer = pkgs.callPackage ./nix/package.nix {
            inherit naerskLib;
            modulixCoreUtilsSrc = modulix-core-utils;
            self = ./.;
          };
          isoImage = self.nixosConfigurations."modulixos-iso-${system}".config.system.build.isoImage;
        in {
          inherit modulixos-installer;
          default = modulixos-installer;
          iso = isoImage;
        });

      nixosModules.installer-kiosk = ./nix/kiosk-module.nix;

      nixosConfigurations = builtins.listToAttrs (map (system: {
        name = "modulixos-iso-${system}";
        value = nixpkgs.lib.nixosSystem {
          inherit system;
          # mxpkgs' modules all call `lib.mkMxDefault`, which only exists on
          # the extended lib — evaluation fails outright without this.
          lib = mxpkgs.lib.extendLib nixpkgs.lib;
          specialArgs = {
            inherit mxpkgs;
            installerPkg = self.packages.${system}.modulixos-installer;
          };
          modules = [ ./nix/iso.nix ];
        };
      }) supportedSystems);
    };
}
