{
  lib,
  config,
  modulesPath,
  mxpkgs,
  ...
}:

let
  release = mxpkgs.lib.modulixRelease;
  asciiCodeName = lib.replaceStrings
    [ "à" "â" "ä" "ç" "é" "è" "ê" "ë" "î" "ï" "ô" "ö" "ù" "û" "ü" " " ]
    [ "a" "a" "a" "c" "e" "e" "e" "e" "i" "i" "o" "o" "u" "u" "u" "-" ]
    release.codeName;
in
{
  imports = [
    "${modulesPath}/installer/cd-dvd/installation-cd-minimal.nix"
    "${mxpkgs}/modulixos/branding.nix"
    "${mxpkgs}/modulixos/boot.nix"
    "${mxpkgs}/modulixos/options"
    ./kiosk-module.nix
  ];

  modulix.installer.enable = true;

  mx.branding = {
    inherit (release) version codeName;
  };
  mx.bootloader.enable = false;

  system.nixos.label = lib.mkForce "${release.version}-${asciiCodeName}";

  isoImage.volumeID = lib.mkForce "MODULIXOS";
  image.baseName = lib.mkForce "modulixos-installer";
  isoImage.appendToMenuLabel = " Installer (kiosk)";
  isoImage.squashfsCompression = "zstd -Xcompression-level 19";

  isoImage.splashImage = config.mx.bootloader.splash.image;
  isoImage.efiSplashImage = config.mx.bootloader.splash.image;
  isoImage.grubTheme = null;

  boot.initrd.kernelModules = [
    "virtio_gpu"
    "qxl"
    "bochs"
    "vmwgfx"
  ];

  boot.tmp.useTmpfs = lib.mkForce false;

  boot.loader.timeout = lib.mkDefault 5;
}
