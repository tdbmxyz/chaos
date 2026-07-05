# ADR 0004 — Users, sessions, and the calendar section

Date: 2026-07-05. Status: accepted.

## Context

The calendar section is the first per-user feature (two users: Tibo and SO),
and the app must keep working logged off — dashboard and links stay
LAN-public, calendars need a signed-in user. An external IdP (authentik) is
planned but not deployed yet; whatever we build now must not need rework
when it arrives, and must work the same from web, desktop and phone apps.

## Decision: identity vs session split

Two layers, deliberately independent:

- **Session layer** (permanent): opaque 244-bit tokens, sha256-hashed in the
  `sessions` table, 90-day expiry. Browsers carry the token in an HttpOnly
  `chaos_session` cookie (set by `POST /auth/login`, same-origin); native
  clients store the token from the login response and send
  `Authorization: Bearer`. `AuthUser` (axum extractor) resolves either form.
- **Identity layer** (pluggable): today, local usernames + argon2id password
  hashes, managed by `chaos-server add-user` (no self-registration — it's a
  two-person household). When authentik lands, an OIDC redirect flow becomes
  a *second way to mint a session*: callback verifies the id_token, maps
  `sub`/`preferred_username` to a `users` row (new column or mapping table),
  creates a session, sets the same cookie. Nothing behind the extractor
  changes; password login can then be disabled per user by leaving
  `password_hash` empty.

Logged-off is a first-class state: only `/auth/me` fails with 401 and the UI
shows sign-in affordances; all pre-existing endpoints remain public on the
LAN. Moving them behind auth later is a one-line change per route
(add the extractor).

## Decision: calendars are per-user, external calendars come in as ICS

- `local` calendars live in SQLite and are writable (event CRUD).
- `ics` calendars subscribe to a feed URL — Google Calendar's "secret
  address", Proton Calendar's share link, or any .ics — fetched server-side,
  cached 10 min, RRULE-expanded per query (rrule crate; unparseable rules
  degrade to the base occurrence). Feeds are read-only.

Why not write to Google/Proton directly: Google's write API requires an
OAuth application + consent flow, Proton has no public calendar API at all.
ICS covers *reading* both today; *writing* happens in chaos-local calendars
(which the household actually shares). If two-way sync is ever needed, the
`kind` enum gains a `caldav` variant — that's the extension point.

All-day events use symbolic UTC-midnight bounds (date semantics), timed
events are real UTC instants displayed in the browser's zone.

## Consequences

- `GET /api/v1/calendar/events?start&end` merges local + feed events; a
  broken feed logs a warning instead of failing the view.
- The session cookie relies on same-origin serving (server serves the web
  bundle; trunk dev proxies /api). CORS stays permissive for bearer-token
  clients; cookies don't cross origins.
- Passwords/tokens never leave the server unhashed; losing chaos.db leaks
  neither.
