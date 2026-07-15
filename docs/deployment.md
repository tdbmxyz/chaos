# Deploying chaos on NixOS (replacing glance)

The flake exposes packages (`chaos-server`, `chaos-web`) and a NixOS module
(`nixosModules.chaos`). The module runs the server as the static `chaos`
system user with state in `/var/lib/chaos` (database, page archives, icon
cache), serves the built frontend, puts `monolith` on the service PATH for
archiving, and installs a `chaos-admin` host command for server-side
administration (user accounts, imports).

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

## Creating user accounts (calendar section)

Calendars are per-user; the dashboard and links work logged off. Accounts
are created on the host with `chaos-admin`, which wraps `chaos-server` with
the service's config and runs it as the `chaos` user (run it as root; DB
migrations apply automatically, and it is safe while the service runs):

```console
# chaos-admin add-user tibo Tibo      # prompts for the password twice
# chaos-admin add-user so "SO"
# chaos-admin list-users
```

Sign in from the web UI (topbar → Sign in). External calendars are added in
the app itself (Calendar → Calendars → “Subscribe to an ICS feed”):

- Google Calendar: Settings → your calendar → “Secret address in iCal
  format”.
- Proton Calendar: share the calendar with a link (read access) and use the
  .ics URL.

Password login is the only identity source today; the session layer is
designed so authentik (OIDC) can be added later without schema changes —
see docs/adr/0004-auth-and-calendar.md.

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

`systemdControl` installs a polkit rule allowing exactly those units and
verbs for the `chaos` service user — no sudo wrappers or sidecar
webservices. The widget config is its own allowlist on top: the HTTP API
refuses units (or actions on `controllable = false` units) that are not
declared on the widget, so the reachable surface is the intersection of
both lists. Without the polkit rule, actions fail with "interactive
authentication required" while status display keeps working.

## Notifications (ntfy)

chaos publishes to an [ntfy](https://ntfy.sh) topic — subscribe with the
ntfy phone app or web UI. Two kinds of pings, both server-side (no web
push, no browser permission dance):

- **Service alerts**: a monitored service going Down/Degraded (or
  recovering) notifies once, after the state survived two polling sweeps —
  flapping services stay silent.
- **Calendar reminders**: events starting within `reminder_lead_minutes`
  (local calendars and ICS feeds, every user) notify once per occurrence.
  All-day events are skipped.

`settings` is free-form, so it is just more TOML:

```nix
services.chaos.settings.notifications = {
  ntfy_url = "https://ntfy.sh";   # or a self-hosted instance
  topic = "chaos-zeus";
  # token = "tk_...";             # only for protected topics
  reminder_lead_minutes = 15;
};
```

Omit the section to keep notifications off. `service_alerts` and
`calendar_reminders` (both default `true`) toggle the halves
independently.

## Backups

Enable scheduled SQLite backups with the `[backup]` section
(`services.chaos.settings.backup` on NixOS):

```nix
services.chaos.settings.backup = {
  enabled = true;
  # dir defaults to "backups" → /var/lib/chaos/backups under the module.
  interval_hours = 24;
  keep = 14;
};
```

Each run writes a consistent `chaos-<timestamp>.db` snapshot via SQLite's
`VACUUM INTO` (safe under WAL, no downtime) and prunes to the `keep`
newest. Restoring = stopping the service and copying a snapshot over
`db_path`. Page archives (`[archive] dir`) and icons are plain files —
include `/var/lib/chaos` in the host's regular backup for those.

## Breaking change: weather proxy removed

`GET /api/v1/weather` is gone in this release — every client (web, desktop,
Android) now geocodes and fetches forecasts directly from Open-Meteo
instead of going through the server. The client and server ship together
(same repo, same release), so there's no compatibility window to worry
about: deploy both at once and there's nothing else to do — the existing
`type = "weather"` config entry keeps working, it just no longer causes a
server-side fetch.

## Desktop and phone apps

The web UI served by the server is the primary interface; the Tauri shells
wrap the same bundle for app-like use:

- **Desktop (NixOS)**: `nix build .#chaos-desktop` (or add the package to
  the system flake) — installs a desktop entry. Point it at the server
  once with `echo http://zeus:4600 > ~/.config/chaos/server` (or the
  `CHAOS_SERVER` env var); without it the app shows a connect screen and
  remembers the address.
- **Desktop (other Linux)**: `just bundle` produces a .deb under
  `target/release/bundle/deb/`.
- **Android**: `just apk` (it enters the `.#android` dev shell itself —
  no Android Studio / ~/Android setup needed); the signed APK lands
  under `crates/chaos-desktop/gen/android/app/build/outputs/apk/`.
  On first launch the connect screen asks for the server address. Release
  signing reads `gen/android/keystore.properties` (see the .sample; the
  real keystore lives in `~/.config/chaos/`).

Shells talk to the server cross-origin, so they sign in with a bearer
token stored on the device instead of the browser cookie; nothing needs
configuring server-side (CORS is already open — LAN posture).

The desktop and Android shells now bundle `tauri-plugin-http` with a
capability allowlist scoped to the hosts the dashboard fetches directly
when offline or for weather (Hacker News, lobste.rs, Open-Meteo). Nothing
to configure — the next `just apk` / `just bundle` build picks it up.

## Migrating Linkwarden data

On the server, with the service stopped or against a copy of the DB:

```console
$ export CHAOS_CONFIG=/nix/store/…-chaos.toml   # or matching CHAOS_* env vars
$ chaos-server import-linkwarden ./linkwarden-export.json [owner-username]
```

The export file comes from Linkwarden → Settings → Data → Export. Links are
queued for archiving automatically when `[archive] auto` is on. The optional
`owner-username` attributes every imported link to that user (`created_by`);
attribution only — every user can still see and edit any link.
