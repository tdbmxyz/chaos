# App OIDC authentication — design

**Date:** 2026-07-25
**Status:** approved

## Problem

A freshly installed app cannot reach the server, and says the wrong thing about
why. Reproduced against the live deployment on 2026-07-25:

```
GET /api/v1/health   Origin: tauri://localhost
→ 302  access-control-allow-origin: tauri://localhost      (traefik's chaos-cors)
       location: https://auth.zeus.balem.fr/application/o/authorize/?…
→ 302  (auth.zeus.balem.fr — no access-control-allow-origin)
→ 200  authentik login page
```

A WebView `fetch` follows redirects transparently, so the chain ends on an
origin that sends no CORS header for `tauri://localhost` and the request fails
as a network error. `offline::probe` sees `ClientError::Transport`, returns
false, and because a fresh install has never marked the server "seen",
`ServerGate` renders **"Cannot reach the chaos server"**. The server is
reachable; the app is unauthenticated. The only place to enter authentik
credentials is the Settings page — behind the gate that just failed.

Three consequences, all of which this design has to fix:

1. **Auth failure is indistinguishable from unreachability.** With
   `redirect: follow` the 302 is not observable by the app at all; it can only
   ever appear as a transport error.
2. **There is no way in from the failing screen.** Even a user who knows about
   app passwords cannot enter them.
3. **A browser login cannot rescue the app.** The outpost cookie is
   `SameSite=Lax` and the app's origin is `tauri://localhost`, so a session
   established in the system browser never rides along on the app's
   cross-origin API calls. Any fix must end in *a token the app holds*.

## Scope discovery: the API is mostly unauthenticated

Only `calendar` (9 handlers) and `views` (3) require `AuthUser` today.
`links`, `collections`, `widgets`, `search`, `home`, `services` and `icons` —
27 handlers — have none. On the public domain, authentik's forward-auth is the
only thing protecting them.

Any design that lets the app past forward-auth therefore has to authenticate
the API itself. That is included here; it is not optional and not deferrable.

## Goals

- One tap to sign in, landing on authentik's own login page (passkeys, 2FA and
  SSO all work because it is really authentik doing the authentication).
- The app stays signed in across app updates.
- Offline: cached reads keep working, and an access token expiring while
  offline does not sign the user out.
- The browser path (forward-auth SSO on `zeus.balem.fr`) keeps working
  unchanged.
- A LAN-only server with no authentik in front keeps working unchanged.

## Non-goals

- The offline **write** outbox (queueing calendar/weather edits made offline
  and syncing on reconnect). Separate sub-project, separate spec; the read half
  already exists in `offline.rs`.
- Moving durable storage of *preferences* off `localStorage`. Separate
  sub-project. This spec makes only the OIDC tokens durable, because "stay
  signed in across updates" depends on it.
- Moving the browser UI to OIDC.
- Android Keystore / OS-keychain storage for the refresh token (see Risks).

## Design

### A. authentik: one new provider (manual, in the UI)

A dedicated OAuth2/OIDC provider for the app, separate from the existing proxy
provider that fronts `zeus.balem.fr`:

| setting | value | why |
| --- | --- | --- |
| client type | **public** | a secret shipped inside an APK is not a secret |
| PKCE | **required, S256** | the only thing binding the code to this app; supported (`code_challenge_methods_supported: ["plain","S256"]`) |
| redirect URI | `xyz.tdbm.chaos://auth/callback` | matches the app id set on 2026-07-25 |
| signing key | **an RSA certificate** | proxy providers sign HS256 and publish an empty `jwks/`; chaos must verify locally |
| scopes | `openid profile email` | `preferred_username` + `name` are what the user mapping needs |
| access token validity | ~1 hour | short, since refresh is silent |
| refresh token validity | 30–90 days | this is what "stays signed in across updates" actually means |

Bound to an authentik application so the usual authorization policies apply.

**Verified available on the live server:** `authorization_code`,
`refresh_token` and the device-code grant are all advertised, and
`code_challenge_methods_supported` includes `S256`.

### B. traefik: one new router, browser path untouched

```
router A   PathPrefix(`/api/v1`) && HeaderRegexp(`Authorization`, `^Bearer `)  → chaos, no forward-auth
router A2  Path(`/api/v1/health`)                                              → chaos, no forward-auth
router B   everything else (existing)                                          → forward-auth, unchanged
```

Router A2 exists so the app can answer "is this server reachable, and does it
want OIDC?" before it holds any token.

Preflight `OPTIONS` carries no `Authorization` header, so it never matches
router A; it falls to router B, where `chaos-cors` already answers preflights
before the forward-auth sees them. The browser is entirely unaffected: its API
calls carry no `Authorization`, so they keep flowing through router B with
proxy headers exactly as today.

Letting any `Bearer`-bearing request skip forward-auth is safe **only** because
of section C. The two ship together.

### C. chaos-server: verify tokens, and require auth everywhere

**Token verification.** New `[oidc]` config block (issuer, client_id, enabled).
JWKS is fetched from the issuer's discovery document and cached in memory with
`kid` lookup and periodic refresh; verification is RS256 with `iss`, `aud`,
`exp` and `nbf` checked. Local verification (rather than authentik's
introspection endpoint) means a request costs no network round trip and keeps
working during a brief authentik outage.

**User mapping.** `preferred_username` → existing chaos user, auto-provisioning
on first sight with `name` as the display name — the same rules
`forward_auth_user` already implements, so an authentik user maps onto the same
chaos account whether they arrive through the browser or the app.

**`AuthUser` gains a third source**, tried in order:

1. `Authorization: Bearer <jwt>` — an OIDC access token (the app)
2. chaos session token or cookie — chaos's own login (unchanged)
3. forward-auth headers — the browser behind authentik (unchanged)

A malformed or expired JWT is a 401, never a fallthrough to a weaker source.

**Every API route requires authentication**, allowlisting exactly `/health` and
`/auth/login`. That is 27 handlers gaining `AuthUser`. A route-coverage test
asserts the allowlist is exhaustive, so a future route added without auth fails
CI rather than silently exposing data.

**`/api/v1/health` advertises how to authenticate:**

```json
{ "status": "ok", "fahrenheit": null,
  "auth": { "oidc": { "issuer": "https://auth.…/application/o/chaos-app/",
                      "client_id": "…",
                      "authorize_url": "https://auth.…/application/o/authorize/" } } }
```

The app self-configures from this: the user types a server address and the
server explains how to sign in. Nothing about authentik is compiled into the
APK, and a server without OIDC simply omits the block — which is also how the
app decides whether to show a "Sign in with authentik" button at all.

### D. The app: tokens live in Rust, not in the WebView

The authorization-code exchange and every refresh happen in the Tauri native
layer, never in JavaScript. Two reasons: authentik will not send
`Access-Control-Allow-Origin: tauri://localhost` for a provider whose redirect
URI is a custom scheme, so a WebView-side exchange would be blocked by CORS;
and the refresh token — the long-lived credential — then never touches WebView
storage.

Commands exposed by `chaos-desktop`:

| command | does |
| --- | --- |
| `auth_start(issuer, client_id)` | generate verifier + state, store them, return the authorize URL |
| `auth_finish(code, state)` | verify state, exchange code + verifier, persist tokens, return the id-token claims |
| `auth_token()` | current access token, refreshed if within 5 minutes of expiry |
| `auth_sign_out()` | drop stored tokens |

`tauri-plugin-deep-link` catches `xyz.tdbm.chaos://auth/callback?code=…&state=…`.
Android needs an intent-filter in `gen/android/app/src/main/AndroidManifest.xml`
(scheme `xyz.tdbm.chaos`, host `auth`, path `/callback`); the Linux desktop
entry generated in `flake.nix` needs
`MimeType=x-scheme-handler/xyz.tdbm.chaos;`.

Tokens persist through `tauri-plugin-store` (a JSON file in the app data
directory), which survives app updates — unlike WebView `localStorage`, which
is what today's app-password fields use.

**The UI side.** `chaos-ui` keeps a module-level access-token signal, populated
on boot and after login by calling `auth_token`, and mirrored into
`localStorage` so a reload has a token synchronously (the refresh token is
never mirrored). `use_client()` reads that mirror, so `chaos-client` needs no
change — it already attaches a Bearer token via `with_token`.

**`ServerGate` becomes three-state:**

| state | when | shows |
| --- | --- | --- |
| Ready | health 200 and (no OIDC advertised, or a token is held) | the app |
| NeedsSignIn | health 200, OIDC advertised, no valid token | "Sign in with authentik" |
| Unreachable | health failed and the server was never seen | today's address form |

Known server that is merely offline keeps booting into the cached UI with the
offline badge, as it does today.

### E. Session lifetime and offline behavior

- Booting offline: cached user, cached data, offline badge. Unchanged.
- **An access token expiring while offline does not sign the user out.** A
  refresh is attempted only when connectivity is Online.
- The session is cleared on exactly three events: explicit sign-out, a refresh
  rejected with `invalid_grant` while online, or `/auth/me` returning 401 while
  online. Today's code drops the session on any API error, which is part of why
  the app "forgets" people.

### F. Migration

The app-password / Basic-auth path is removed once OIDC works: the Settings
fields, `AUTHENTIK_USER_KEY`/`AUTHENTIK_TOKEN_KEY`, and
`ChaosClient::with_basic_auth`. Two credential systems in one app is the
confusion this project exists to end. The server-side forward-auth header path
stays — that is the browser's, not the app's.

Rollout order matters: **C before B**. If traefik lets Bearer requests past
forward-auth before chaos requires auth on every route, the API is briefly
open. Deploying C first is harmless (nothing sends a Bearer yet).

## Testing

Server (`cargo nextest run`):

- JWT verification: valid, expired, wrong issuer, wrong audience, bad
  signature, unknown `kid` — each maps to the expected 401 or success.
- Claim → user mapping, including auto-provisioning on first sight and reuse of
  the existing account on the second.
- `AuthUser` source precedence, including that a malformed Bearer is a 401
  rather than a fallthrough to forward-auth headers.
- Route coverage: every route in the router requires auth except `/health` and
  `/auth/login`.
- `/api/v1/health` advertises the `auth` block when configured and omits it
  when not.

App:

- PKCE challenge derivation (`S256` = base64url(sha256(verifier))) against
  RFC 7636's published test vector.
- State mismatch rejects the callback.
- Token refresh triggers within the expiry window and not outside it.
- Gate state selection: a pure function over (health result, OIDC advertised,
  token held, server seen) → the four states.

Manual, on device (no automation possible):

- Fresh install → address → "Sign in with authentik" → browser → back into the
  app signed in.
- Airplane mode → app still opens with cached data and does not sign out;
  reconnect → still signed in.
- Reinstall the APK over an existing install → still signed in.

## Risks

- **The Bearer-bypass router is only as safe as chaos's auth coverage.**
  Mitigated by the route-coverage test and by deploying C before B.
- **Refresh token at rest.** `tauri-plugin-store` is a plain file in the app
  data directory: safe from other apps under Android's sandbox, readable by
  anyone with root or a device backup. Android Keystore is meaningfully
  stronger and meaningfully more work; deliberately deferred, and noted so it
  can be revisited.
- **Desktop deep links can spawn a second instance.** Linux hands the URL to a
  fresh process; without `tauri-plugin-single-instance` the callback may land
  in an instance that never started the flow. Android is unaffected. If this
  proves awkward, the desktop can fall back to the device-code grant, which
  needs no redirect at all and is already supported by the server-side provider.
- **A user's authentik account not matching their chaos username** provisions a
  second chaos account, exactly as forward-auth does today (this is how
  `akadmin` appeared). Unchanged behavior, called out because OIDC makes it
  easier to hit.

## What the user has to do by hand

1. Create the authentik provider + application per section A and note the
   client_id.
2. Add `[oidc]` to the chaos config in `/etc/nixos` and the two traefik routers
   from section B (this session applies but never commits `/etc/nixos`).
3. `nixos-rebuild switch`, then install the new APK.
