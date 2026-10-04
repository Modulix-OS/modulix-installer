{
  lib,
  pkgs,
  naerskLib,
  modulixCoreUtilsSrc,
  self,
}:

let
  mergedSrc = pkgs.runCommand "modulixos-installer-src" { } ''
    mkdir -p $out
    cp -r ${self} $out/modulixos-installer
    cp -r ${modulixCoreUtilsSrc} $out/modulix-core-utils
    chmod -R u+w $out
  '';
in
naerskLib.buildPackage {
  src = mergedSrc;
  root = "${mergedSrc}/modulixos-installer";
  singleStep = true;
  cargoBuildOptions = def: def ++ [ "--manifest-path" "modulixos-installer/Cargo.toml" ];
  cargoTestOptions = def: def ++ [ "--manifest-path" "modulixos-installer/Cargo.toml" ];

  nativeBuildInputs = with pkgs; [
    pkg-config
    glib
    gettext
    wrapGAppsHook4
    makeWrapper
  ];

  buildInputs = with pkgs; [
    glib
    gtk4
    libadwaita
    webkitgtk_6_0
    zlib

    adwaita-icon-theme
    hicolor-icon-theme
    gsettings-desktop-schemas
  ];

  env.PAPIRUS_ICON_THEME_48 = "${pkgs.papirus-icon-theme}/share/icons/Papirus/48x48";

  dontWrapGApps = true;

  doCheck = true;

  postInstall = ''
    for po in ${mergedSrc}/modulixos-installer/po/*.po; do
      lang=$(basename "$po" .po)
      dest="$out/share/locale/$lang/LC_MESSAGES/modulixos-installer.mo"
      mkdir -p "$(dirname "$dest")"
      ${pkgs.gettext}/bin/msgfmt "$po" -o "$dest"
    done
  '';

  postFixup = ''
    wrapProgram $out/bin/modulixos-installer \
      "''${gappsWrapperArgs[@]}" \
      --prefix PATH : ${
        lib.makeBinPath (
          with pkgs;
          [
            util-linux
            cryptsetup
            tpm2-tools
            systemd
            efibootmgr
            ntfs3g
            e2fsprogs
            gptfdisk
            gparted
            gnome-calculator
            glib
            git
            sway
            orca
            speechd
          ]
        )
      } \
      --suffix PATH : ${
        lib.makeBinPath (
          with pkgs;
          [
            nixos-install-tools
            pciutils
            usbutils
            cpuid
            nix
          ]
        )
      } \
      --prefix XDG_DATA_DIRS : ${pkgs.adwaita-icon-theme}/share \
      --prefix XDG_DATA_DIRS : ${pkgs.hicolor-icon-theme}/share \
      --set MODULIX_LOCALE_DIR $out/share/locale \
      --set TZDIR ${pkgs.tzdata}/share/zoneinfo \
      --set MODULIX_DEV_EVDEV_XML ${pkgs.xkeyboard_config}/share/X11/xkb/rules/evdev.xml \
      --set MODULIX_XKB_LOCALE_DIR ${pkgs.xkeyboard_config}/share/locale \
      --set MODULIX_DEV_LOCALE_SUPPORTED ${pkgs.glibcLocales}/share/i18n/SUPPORTED
  '';

  meta = with lib; {
    description = "Fullscreen GTK4/libadwaita installer for Modulix OS";
    mainProgram = "modulixos-installer";
    platforms = platforms.linux;
  };
}
