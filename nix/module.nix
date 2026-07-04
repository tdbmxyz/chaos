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

      # The archiver shells out to monolith.
      path = [cfg.monolithPackage];

      environment.CHAOS_CONFIG = configFile;

      serviceConfig = {
        ExecStart = lib.getExe cfg.package;
        DynamicUser = true;
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

    networking.firewall.allowedTCPPorts = lib.mkIf cfg.openFirewall [cfg.port];
  };
}
