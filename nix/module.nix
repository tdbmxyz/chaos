# NixOS module for the chaos dashboard. Imported from the flake as
# `nixosModules.chaos`; `self` provides the default packages.
self: {
  config,
  lib,
  pkgs,
  ...
}: let
  cfg = config.services.chaos;
  settingsFormat = pkgs.formats.toml {};
  configFile = settingsFormat.generate "chaos.toml" cfg.settings;
in {
  options.services.chaos = {
    enable = lib.mkEnableOption "chaos, the unified dashboard for local services";

    package = lib.mkOption {
      type = lib.types.package;
      default = self.packages.${pkgs.stdenv.hostPlatform.system}.chaos-server;
      defaultText = lib.literalExpression "chaos.packages.\${system}.chaos-server";
      description = "chaos-server package to run.";
    };

    webPackage = lib.mkOption {
      type = lib.types.nullOr lib.types.package;
      default = self.packages.${pkgs.stdenv.hostPlatform.system}.chaos-web;
      defaultText = lib.literalExpression "chaos.packages.\${system}.chaos-web";
      description = "Built web frontend served by the server (null to disable).";
    };

    monolithPackage = lib.mkOption {
      type = lib.types.package;
      default = pkgs.monolith;
      defaultText = lib.literalExpression "pkgs.monolith";
      description = "monolith package used for page snapshots.";
    };

    address = lib.mkOption {
      type = lib.types.str;
      default = "0.0.0.0";
      description = "Address to bind to.";
    };

    port = lib.mkOption {
      type = lib.types.port;
      default = 4600;
      description = "Port to listen on.";
    };

    openFirewall = lib.mkOption {
      type = lib.types.bool;
      default = false;
      description = "Open the chaos port in the firewall.";
    };

    systemdControl = {
      enable = lib.mkEnableOption "control of systemd units from the chaos dashboard";

      units = lib.mkOption {
        type = lib.types.listOf lib.types.str;
        default = [];
        example = ["stirling-pdf.service" "sunshine.service"];
        description = ''
          Units the chaos server may start/stop/restart (for the `systemd`
          dashboard widget). A polkit rule is installed that allows exactly
          these units and verbs for the chaos user; the widget config in
          `settings` should list the same units.
        '';
      };
    };

    settings = lib.mkOption {
      type = settingsFormat.type;
      default = {};
      example = lib.literalExpression ''
        {
          services = [
            {
              id = "jellyfin";
              title = "Jellyfin";
              url = "http://zeus:8096";
              icon = "di:jellyfin";
            }
          ];
          bookmarks = [
            {
              title = "Main";
              links = [
                {
                  title = "GitHub";
                  url = "https://github.com";
                  icon = "si:github";
                }
              ];
            }
          ];
        }
      '';
      description = ''
        chaos configuration, serialized to chaos.toml. See
        crates/chaos-server/chaos.example.toml for the available keys.
        State paths (db_path, archive.dir) and listen default to sane
        values under /var/lib/chaos and can be overridden here.
      '';
    };
  };

  config = lib.mkIf cfg.enable {
    services.chaos.settings = {
      listen = lib.mkDefault "${cfg.address}:${toString cfg.port}";
      db_path = lib.mkDefault "/var/lib/chaos/chaos.db";
      archive.dir = lib.mkDefault "/var/lib/chaos/archives";
      icon_cache_dir = lib.mkDefault "/var/lib/chaos/icons";
      static_dir = lib.mkIf (cfg.webPackage != null) (lib.mkDefault cfg.webPackage);
    };

    systemd.services.chaos = {
      description = "chaos dashboard";
      wantedBy = ["multi-user.target"];
      after = ["network-online.target"];
      wants = ["network-online.target"];

      # The archiver shells out to monolith; the systemd widget to systemctl.
      path = [cfg.monolithPackage] ++ lib.optional cfg.systemdControl.enable pkgs.systemd;

      environment.CHAOS_CONFIG = configFile;

      serviceConfig = {
        ExecStart = lib.getExe cfg.package;
        # polkit rules match on the user name, so unit control needs a
        # static identity instead of a dynamic one.
        DynamicUser = !cfg.systemdControl.enable;
        User = lib.mkIf cfg.systemdControl.enable "chaos";
        Group = lib.mkIf cfg.systemdControl.enable "chaos";
        StateDirectory = "chaos";
        WorkingDirectory = "/var/lib/chaos";
        Restart = "on-failure";
        RestartSec = 5;

        # Hardening (the service only needs its state dir and the network).
        NoNewPrivileges = true;
        PrivateTmp = true;
        ProtectSystem = "strict";
        ProtectHome = true;
        ProtectKernelTunables = true;
        ProtectControlGroups = true;
        RestrictSUIDSGID = true;
        LockPersonality = true;
      };
    };

    users.users.chaos = lib.mkIf cfg.systemdControl.enable {
      isSystemUser = true;
      group = "chaos";
    };
    users.groups.chaos = lib.mkIf cfg.systemdControl.enable {};

    # systemctl authorizes non-root callers through polkit; allow exactly
    # the configured units and verbs for the chaos user.
    security.polkit = lib.mkIf cfg.systemdControl.enable {
      enable = true;
      extraConfig = ''
        polkit.addRule(function (action, subject) {
          if (
            action.id == "org.freedesktop.systemd1.manage-units" &&
            subject.user == "chaos" &&
            ${builtins.toJSON cfg.systemdControl.units}.indexOf(action.lookup("unit")) >= 0 &&
            ["start", "stop", "restart"].indexOf(action.lookup("verb")) >= 0
          ) {
            return polkit.Result.YES;
          }
        });
      '';
    };

    assertions = [
      {
        assertion = cfg.systemdControl.enable -> cfg.systemdControl.units != [];
        message = "services.chaos.systemdControl.enable requires systemdControl.units to be non-empty.";
      }
    ];

    networking.firewall.allowedTCPPorts = lib.mkIf cfg.openFirewall [cfg.port];
  };
}
