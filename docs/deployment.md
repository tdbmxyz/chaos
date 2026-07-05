# Deploying chaos on NixOS (replacing glance)

The flake exposes packages (`chaos-server`, `chaos-web`) and a NixOS module
(`nixosModules.chaos`). The module runs the server under a dynamic user with
state in `/var/lib/chaos` (database, page archives, icon cache), serves the
built frontend, and puts `monolith` on the service PATH for archiving.

## System flake wiring

```nix
{
  inputs.chaos.url = "github:tibo/chaos"; # or a local path during testing

  # in the host's modules:
  imports = [ inputs.chaos.nixosModules.chaos ];
}
```

## Replacing inspirations/glance.nix

The old glance module built `monitorSites` from
`config.modules.server.servicesList`. The same pattern maps directly onto
`services.chaos.settings.services`:

```nix
{ config, lib, ... }: let
  domain = config.networking.hostName;
  customServices = config.modules.server.servicesList;

  serviceSites = [
    { service = "jellyfin"; icon = "di:jellyfin"; title = "Jellyfin"; }
    { service = "immich";   icon = "di:immich";   title = "Immich"; }
    # … same list as glance.nix
  ];

  monitorServices = map (site: {
    id = site.service;
    inherit (site) icon title;
    url = "http://${domain}:${toString customServices.${site.service}.remotePort}";
  }) (builtins.filter (site: customServices ? ${site.service}) serviceSites);
in {
  services.chaos = {
    enable = true;
    port = 4600;
    openFirewall = true;

    settings = {
      search_url = "https://duckduckgo.com/?q={}"; # or local SearXNG

      services = monitorServices;

      bookmarks = [
        {
          title = "Main";
          links = [
            { title = "ProtonMail"; url = "https://mail.proton.me"; icon = "si:protonmail"; }
            { title = "GitHub";     url = "https://github.com";     icon = "si:github"; }
            { title = "Reddit";     url = "https://reddit.com";     icon = "si:reddit"; }
          ];
        }
      ];
    };
  };
}
```

Icon conventions are the glance ones: `di:` (dashboard-icons), `si:`
(Simple Icons), `sh:` (selfh.st) or a plain URL. The server fetches and
caches them in `/var/lib/chaos/icons`, so browsers on the LAN never need
internet access for icons.

## Options summary

| Option | Default | Purpose |
| --- | --- | --- |
| `services.chaos.enable` | `false` | Turn the service on |
| `services.chaos.port` / `.address` | `4600` / `0.0.0.0` | Listen address |
| `services.chaos.openFirewall` | `false` | Open the port |
| `services.chaos.package` | flake's `chaos-server` | Server binary |
| `services.chaos.webPackage` | flake's `chaos-web` | Frontend (null = API only) |
| `services.chaos.monolithPackage` | `pkgs.monolith` | Archiver binary |
| `services.chaos.settings` | `{}` | Free-form chaos.toml (see example config) |
| `services.chaos.systemdControl.enable` | `false` | Allow unit control from the dashboard |
| `services.chaos.systemdControl.units` | `[]` | Units the server may start/stop/restart |

`settings` is serialized verbatim to chaos.toml; anything the server
understands (`[archive]`, `[monitor]`, `search_url`, `services`,
`bookmarks`, `columns`…) goes there. The module only injects defaults for
`listen` and the state paths.

## Controlling systemd units (services manager widget)

The `systemd` dashboard widget shows unit states and, for controllable
units, start/stop/restart buttons. Two halves must agree:

```nix
services.chaos = {
  systemdControl = {
    enable = true;                          # polkit rule + static chaos user
    units = ["stirling-pdf.service" "sunshine.service"];
  };
  settings.columns = [
    # …
    {
      size = "small";
      widgets = [
        {
          type = "systemd";
          title = "Machines";
          units = [
            { unit = "stirling-pdf.service"; title = "Stirling PDF"; }
            { unit = "sunshine.service"; title = "Remote desktop"; }
          ];
        }
      ];
    }
  ];
};
```

`systemdControl` swaps DynamicUser for a static `chaos` user and installs a
polkit rule allowing exactly those units and verbs for that user — no sudo
wrappers or sidecar webservices. The widget config is its own allowlist on
top: the HTTP API refuses units (or actions on `controllable = false`
units) that are not declared on the widget, so the reachable surface is
the intersection of both lists. Without the polkit rule, actions fail with
"interactive authentication required" while status display keeps working.

## Migrating Linkwarden data

On the server, with the service stopped or against a copy of the DB:

```console
$ export CHAOS_CONFIG=/nix/store/…-chaos.toml   # or matching CHAOS_* env vars
$ chaos-server import-linkwarden ./linkwarden-export.json
```

The export file comes from Linkwarden → Settings → Data → Export. Links are
queued for archiving automatically when `[archive] auto` is on.
