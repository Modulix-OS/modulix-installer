{
  description = "modulixos-installer — fullscreen GTK4/libadwaita installer for Modulix OS";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    naersk.url = "github:nix-community/naersk";
    naersk.inputs.nixpkgs.follows = "nixpkgs";
    mxpkgs.url = "github:Modulix-OS/mxpkgs";
  };

  outputs = { self, nixpkgs, naersk, mxpkgs }:
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
              glib
              gettext
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
              pciutils
              usbutils
              cpuid
              ntfs3g
              gptfdisk
              util-linux
              cryptsetup
              tpm2-tools
              udisks2
              gparted
              gnome-calculator
              papirus-icon-theme
              tzdata
              glibcLocales
              xkeyboard_config
              e2fsprogs
              sway
              orca
              speechd
            ];
            RUST_SRC_PATH = "${pkgs.rust.packages.stable.rustPlatform.rustLibSrc}";
            TZDIR = "${pkgs.tzdata}/share/zoneinfo";
            PAPIRUS_ICON_THEME_48 = "${pkgs.papirus-icon-theme}/share/icons/Papirus/48x48";
            MODULIX_DEV_EVDEV_XML = "${pkgs.xkeyboard_config}/share/X11/xkb/rules/evdev.xml";
            MODULIX_XKB_LOCALE_DIR = "${pkgs.xkeyboard_config}/share/locale";
            MODULIX_DEV_LOCALE_SUPPORTED = "${pkgs.glibcLocales}/share/i18n/SUPPORTED";
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
            self = ./.;
          };
          modulixos-installer-debug = pkgs.callPackage ./nix/package.nix {
            inherit naerskLib;
            self = ./.;
            release = false;
          };
          rawIsoImage = self.nixosConfigurations."modulixos-iso-${system}".config.system.build.isoImage;
        in {
          inherit modulixos-installer modulixos-installer-debug;
          default = modulixos-installer;
          iso = pkgs.callPackage ./nix/iso-checksum.nix { isoImage = rawIsoImage; };
        });

      nixosModules.installer-kiosk = ./nix/kiosk-module.nix;

      nixosConfigurations = builtins.listToAttrs (map (system: {
        name = "modulixos-iso-${system}";
        value = nixpkgs.lib.nixosSystem {
          inherit system;
          lib = mxpkgs.lib.extendLib nixpkgs.lib;
          specialArgs = {
            inherit mxpkgs;
            installerPkg = self.packages.${system}.modulixos-installer;
          };
          modules = [ ./nix/iso.nix ];
        };
      }) supportedSystems);

      apps = forAllSystems (system:
        let
          pkgs = pkgsFor system;
        in {
          vm = {
            type = "app";
            program = "${pkgs.callPackage ./nix/vm.nix {
              isoImage = self.packages.${system}.iso;
            }}/bin/modulixos-vm";
          };
        });
    };
}
