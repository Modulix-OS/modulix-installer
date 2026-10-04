{
  config,
  lib,
  pkgs,
  utils,
  installerPkg,
  ...
}:

let
  cfg = config.modulix.installer;
  installerStartedFlag = "/run/modulix-installer.started";
  installerStatusFile = "/run/modulix-installer.status";

  runInstaller = pkgs.writeShellScript "modulix-installer-run" ''
    ${applyOutputScale} || true
    ${pkgs.coreutils}/bin/touch ${installerStartedFlag}
    status=0
    ${installerPkg}/bin/modulixos-installer || status=$?
    ${pkgs.coreutils}/bin/printf '%s' "$status" > ${installerStatusFile}
    [ "$status" -eq 0 ] || echo "modulixos-installer exited with status $status" >&2
    # The installer is sway's only client — leaving the compositor up would
    # just show an empty screen.
    ${pkgs.sway}/bin/swaymsg exit || true
  '';

  applyOutputScale = pkgs.writeShellScript "modulix-apply-output-scale" ''
    set -u

    jq=${pkgs.jq}/bin/jq
    swaymsg=${pkgs.sway}/bin/swaymsg
    od=${pkgs.coreutils}/bin/od
    printf_=${pkgs.coreutils}/bin/printf

    min_logical_w=1280
    min_logical_h=800
    max_logical_h=1300

    override=""
    if [ -n "''${MODULIX_SCALE:-}" ]; then
      override="$MODULIX_SCALE"
    else
      for arg in $(${pkgs.coreutils}/bin/cat /proc/cmdline); do
        case "$arg" in
          modulix.scale=*) override="''${arg#modulix.scale=}" ;;
        esac
      done
    fi

    is_virt=0
    ${pkgs.systemd}/bin/systemd-detect-virt --quiet && is_virt=1

    fmt_scale() {
      "$printf_" '%d.%02d' $(($1 / 100)) $(($1 % 100))
    }

    edid_width_cm() {
      connector="$1"
      for f in /sys/class/drm/card*-"$connector"/edid; do
        [ -s "$f" ] || continue
        set -- $("$od" -An -tu1 -j21 -N1 "$f")
        echo "''${1:-0}"
        return
      done
      echo 0
    }

    "$swaymsg" -t get_outputs -r \
      | "$jq" -r '.[] | select(.active) | "\(.name) \(.current_mode.width) \(.current_mode.height)"' \
      | while read -r name w h; do
      [ -n "$name" ] || continue
      case "$w$h" in *[!0-9]* | "") continue ;; esac
      [ "$w" -gt 0 ] && [ "$h" -gt 0 ] || continue

      if [ -n "$override" ]; then
        echo "modulix scale: $name forced to $override" >&2
        "$swaymsg" output "$name" scale "$override" >/dev/null || true
        continue
      fi

      cm=0
      [ "$is_virt" -eq 1 ] || cm=$(edid_width_cm "$name")
      case "$cm" in *[!0-9]* | "") cm=0 ;; esac

      dpi=0
      [ "$cm" -gt 0 ] && dpi=$((w * 254 / (cm * 100)))

      if [ "$dpi" -ge 120 ]; then
        raw=$((dpi * 100 / 96))
        cents=$(((raw + 12) / 25 * 25))
      elif [ "$dpi" -gt 0 ]; then
        cents=100
      elif [ "$w" -ge 3200 ] || [ "$h" -ge 1800 ]; then
        cents=200
      elif [ "$w" -ge 2400 ] || [ "$h" -ge 1400 ]; then
        cents=150
      else
        cents=100
      fi

      [ "$cents" -le 300 ] || cents=300
      [ "$cents" -ge 100 ] || cents=100

      while [ "$cents" -lt 300 ] && [ $((h * 100 / cents)) -gt "$max_logical_h" ]; do
        cents=$((cents + 25))
      done

      while [ "$cents" -gt 100 ] \
        && { [ $((w * 100 / cents)) -lt "$min_logical_w" ] \
          || [ $((h * 100 / cents)) -lt "$min_logical_h" ]; }; do
        cents=$((cents - 25))
      done

      scale=$(fmt_scale "$cents")
      echo "modulix scale: $name ''${w}x''${h} ''${dpi}dpi -> scale $scale" >&2
      "$swaymsg" output "$name" scale "$scale" >/dev/null || true
    done
  '';

  waitForGpu = pkgs.writeShellScript "modulix-wait-for-gpu" ''
    ${config.systemd.package}/bin/udevadm settle --timeout=30 || true
    for _ in $(${pkgs.coreutils}/bin/seq 1 150); do
      if ${pkgs.coreutils}/bin/ls /dev/dri/renderD* >/dev/null 2>&1; then
        exit 0
      fi
      ${pkgs.coreutils}/bin/sleep 0.1
    done
    echo "no DRM render node after udev settle + 15s; continuing on whatever card exists" >&2
    exit 0
  '';

  dumpFailure = pkgs.writeShellScript "modulix-installer-dump-failure" ''
    exec > /dev/tty1 2>&1
    if [ -e /run/modulix-installer.dumped ]; then exit 0; fi
    ${pkgs.coreutils}/bin/touch /run/modulix-installer.dumped
    ${pkgs.kbd}/bin/chvt 1 || true
    ${pkgs.util-linux}/bin/setterm --blank 0 --powersave off --cursor on || true
    echo
    echo "=============================================================="
    if ${config.systemd.package}/bin/systemctl is-failed --quiet modulix-installer-session.service; then
      echo " Modulix OS: the graphical installer session FAILED to start."
    else
      echo " Modulix OS: the graphical installer session has ended."
    fi
    echo "=============================================================="
    echo
    ${config.systemd.package}/bin/systemctl --no-pager --full status \
      modulix-installer-session.service || true
    echo
    echo "--- last 60 journal lines of the session ---------------------"
    ${config.systemd.package}/bin/journalctl -b -u modulix-installer-session --no-pager \
      | ${pkgs.coreutils}/bin/tail -n 60
    echo
    echo "--- DRM devices ----------------------------------------------"
    ${pkgs.coreutils}/bin/ls -l /dev/dri/ 2>&1 || true
    echo
    echo "Full boot log:     journalctl -b --no-pager"
    echo "Installer log:     /var/log/modulixos-install.log (if the install started)"
    echo "Copy to a USB key: lsblk; mount /dev/sdXN /mnt2;"
    echo "                   journalctl -b --no-pager > /mnt2/modulix-boot.log; umount /mnt2"
    echo
  '';

  startSession = pkgs.writeShellScript "modulix-installer-session" ''
    export WLR_NO_HARDWARE_CURSORS=1
    ${pkgs.coreutils}/bin/rm -f ${installerStartedFlag} ${installerStatusFile}

    software_render() {
      export WLR_RENDERER=pixman
      export GSK_RENDERER=cairo
      export WEBKIT_DISABLE_DMABUF_RENDERER=1
    }
    run_sway() {
      ${pkgs.dbus}/bin/dbus-run-session ${pkgs.sway}/bin/sway -c ${swayConfig}
    }
    run_sway_debug() {
      ${pkgs.dbus}/bin/dbus-run-session ${pkgs.sway}/bin/sway -d -c ${swayConfig}
    }

    if ${pkgs.systemd}/bin/systemd-detect-virt --quiet; then
      software_render
    elif ! ${pkgs.coreutils}/bin/ls /dev/dri/renderD* >/dev/null 2>&1; then
      echo "no DRM render node, going straight to the software renderer" >&2
      software_render
    fi

    status=0
    run_sway || status=$?
    if [ ! -e ${installerStartedFlag} ] && [ -z "''${WLR_RENDERER:-}" ]; then
      echo "sway exited with status $status before the installer ever started, retrying in software" >&2
      software_render
      status=0
      run_sway_debug || status=$?
    fi

    installer_status=$(${pkgs.coreutils}/bin/cat ${installerStatusFile} 2>/dev/null || echo missing)
    if [ "$status" -ne 0 ] || [ "$installer_status" != 0 ]; then
      echo "kiosk session failed (sway=$status installer=$installer_status)" >&2
      exit 1
    fi
  '';

  swayConfig = pkgs.writeText "modulix-installer-sway.conf" ''
    output * bg #241f31 solid_color
    default_border none
    default_floating_border none
    seat * hide_cursor 8000

    # Starting layout only: the keyboard step rewrites it live over the sway
    # IPC socket (see src/backend/locale/system.rs).
    input type:keyboard {
        xkb_layout us
    }

    # The installer's lists (locales, timezones, keyboard layouts) are long and
    # libinput's default touchpad scroll overshoots them by several screens per
    # flick. Slowing it down here covers every window of the session, GParted
    # included.
    input type:touchpad {
        scroll_factor 0.4
    }

    # GParted (manual partitioning) and the captive-portal window are second
    # toplevels — give them the whole screen instead of a tiled half.
    for_window [app_id="gparted"] fullscreen enable
    for_window [title="GParted"] fullscreen enable

    exec ${runInstaller}
  '';
in
{
  options.modulix.installer.enable = lib.mkEnableOption "the modulixos-installer sway kiosk session";

  config = lib.mkIf cfg.enable {
    systemd.services."modulix-installer-session" = {
      enable = true;
      after = [
        "systemd-user-sessions.service"
        "plymouth-start.service"
        "plymouth-quit.service"
        "plymouth-quit-wait.service"
        "systemd-logind.service"
        "getty@tty1.service"
      ];
      before = [ "graphical.target" ];
      wants = [
        "dbus.socket"
        "systemd-logind.service"
        "plymouth-quit.service"
        "plymouth-quit-wait.service"
      ];
      wantedBy = [ "graphical.target" ];
      conflicts = [
        "getty@tty1.service"
        "modulix-installer-fallback.service"
      ];
      onFailure = [ "modulix-installer-fallback.service" ];
      onSuccess = [ "modulix-installer-fallback.service" ];

      restartIfChanged = false;
      unitConfig.AssertPathExists = "/dev/tty1";

      path = [ config.system.path ];

      environment = {
        LOCALE_ARCHIVE = "/run/current-system/sw/lib/locale/locale-archive";
        # WLR_RENDERER_ALLOW_SOFTWARE = "1";
      };

      serviceConfig = {
        ExecStartPre = "${waitForGpu}";
        ExecStart = "${startSession}";
        User = "root";

        IgnoreSIGPIPE = "no";
        UtmpIdentifier = "%n";
        UtmpMode = "user";
        TTYPath = "/dev/tty1";
        TTYReset = "yes";
        TTYVHangup = "yes";
        TTYVTDisallocate = "no";
        StandardInput = "tty-fail";
        StandardOutput = "journal+console";
        StandardError = "journal+console";
        PAMName = "modulix-installer-session";
      };
    };

    systemd.services."modulix-installer-fallback" = {
      description = "Recovery console on tty1 for the modulixos-installer session";
      after = [ "modulix-installer-session.service" ];
      conflicts = [ "modulix-installer-session.service" ];
      unitConfig.ConditionPathExists = "/dev/tty1";
      restartIfChanged = false;

      serviceConfig = {
        Type = "idle";
        ExecStartPre = "${dumpFailure}";
        ExecStart = "${pkgs.util-linux}/bin/agetty --autologin root --noclear --keep-baud tty1 115200,38400,9600";
        Restart = "always";
        RestartSec = 1;
        TTYPath = "/dev/tty1";
        TTYReset = "yes";
        TTYVHangup = "yes";
        StandardInput = "tty";
        StandardOutput = "tty";
        StandardError = "journal";
        UtmpIdentifier = "%n";
        UtmpMode = "user";
      };
    };

    security.pam.services."modulix-installer-session" = {
      useDefaultRules = false;
      rules = {
        auth = utils.pam.autoOrderRules [
          {
            name = "unix";
            control = "required";
            modulePath = "${config.security.pam.package}/lib/security/pam_unix.so";
            settings.nullok = true;
          }
        ];
        account = utils.pam.autoOrderRules [
          {
            name = "unix";
            control = "required";
            modulePath = "${config.security.pam.package}/lib/security/pam_unix.so";
          }
        ];
        session = utils.pam.autoOrderRules [
          {
            name = "unix";
            control = "required";
            modulePath = "${config.security.pam.package}/lib/security/pam_unix.so";
          }
          {
            name = "env";
            control = "required";
            modulePath = "${config.security.pam.package}/lib/security/pam_env.so";
            settings.conffile = "/etc/pam/environment";
            settings.readenv = 0;
          }
          {
            name = "systemd";
            control = "required";
            modulePath = "${config.systemd.package}/lib/security/pam_systemd.so";
          }
        ];
      };
    };

    systemd.defaultUnit = "graphical.target";

    i18n.supportedLocales = [
      "C.UTF-8/UTF-8"
      "en_US.UTF-8/UTF-8"
      "fr_FR.UTF-8/UTF-8"
      "es_ES.UTF-8/UTF-8"
      "de_DE.UTF-8/UTF-8"
    ];

    nix.settings.experimental-features = [
      "nix-command"
      "flakes"
    ];

    services.udisks2.enable = true;
    networking.networkmanager.enable = true;

    services.speechd.enable = true;
    services.gnome.at-spi2-core.enable = true;

    hardware.graphics.enable = true;
    fonts.packages = with pkgs; [
      cantarell-fonts
      dejavu_fonts
      noto-fonts
      noto-fonts-emoji
    ];

    security.polkit.enable = true;

    environment.systemPackages = [
      installerPkg
    ]
    ++ (with pkgs; [
      util-linux
      cryptsetup
      tpm2-tools
      efibootmgr
      ntfs3g
      e2fsprogs
      gptfdisk
      gparted
      gnome-calculator
      git
      jq
      sway
      orca
      speechd
      udisks2
      kbd
      pciutils
      usbutils
      cpuid
    ]);

    systemd.services."getty@tty1".enable = false;
    systemd.services."autovt@tty1".enable = false;
    systemd.targets.getty.wants = lib.mkForce [ ];
  };
}
