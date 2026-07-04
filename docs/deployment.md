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

`settings` is serialized verbatim to chaos.toml; anything the server
understands (`[archive]`, `[monitor]`, `search_url`, `services`,
`bookmarks`…) goes there. The module only injects defaults for `listen`
and the state paths.

## Migrating Linkwarden data

On the server, with the service stopped or against a copy of the DB:

```console
$ export CHAOS_CONFIG=/nix/store/…-chaos.toml   # or matching CHAOS_* env vars
$ chaos-server import-linkwarden ./linkwarden-export.json
```

The export file comes from Linkwarden → Settings → Data → Export. Links are
queued for archiving automatically when `[archive] auto` is on.
