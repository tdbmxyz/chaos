# Rolling out app sign-in (OIDC)

What to do, in this order, to switch the mobile and desktop apps from the
app-password hack to a real authentik sign-in. Everything here is a manual
step: the authentik provider can only be created in its UI, and `/etc/nixos`
belongs to you.

Background and the reasoning behind each piece:
`docs/superpowers/specs/2026-07-25-app-oidc-auth-design.md`.

**The browser path is untouched at every step below.** `zeus.balem.fr` keeps
its forward-auth SSO whether or not any of this is done, so there is no window
where the web UI stops working.

---

## 1. Create the authentik provider

Applications → Providers → Create → **OAuth2/OpenID Provider**.

| setting | value | why it matters |
| --- | --- | --- |
| Name | `chaos-app` | |
| Authorization flow | your usual explicit-consent or implicit-consent flow | |
| Client type | **Public** | a secret shipped in an APK is not a secret |
| Client ID | *(copy it — you need it in step 3)* | |
| Redirect URI | `xyz.tdbm.chaos://auth/callback` | must match `REDIRECT_URI` in `crates/chaos-desktop/src/auth.rs` exactly |
| Signing Key | **an RSA certificate** (e.g. `authentik Self-signed Certificate`) | ← the one that silently breaks everything if wrong; see below |
| Scopes | `openid`, `profile`, `email` | `preferred_username` and `name` are what map you onto your chaos account |
| Access code validity | leave default (minutes) | |
| Access token validity | ~1 hour | refresh is silent, so short is free |
| Refresh token validity | 30–90 days | this is what "stays signed in across app updates" means in practice |

Then Applications → Create, bind it to this provider, and set whatever
authorization policy you want — that policy is what decides who can use the
app.

**PKCE:** authentik requires it for public clients automatically. If your
version exposes a "PKCE required" toggle, turn it on.

### Why the signing key matters

chaos verifies tokens locally against the provider's JWKS, so it needs an
asymmetric key. Proxy providers sign HS256 and publish an empty key set — the
existing one on this deployment does exactly that:

```console
$ curl -s https://auth.zeus.balem.fr/application/o/chaos/jwks/
{}
```

A provider like that would make every app request fail verification.

## 2. Verify the provider before changing anything else

Take the issuer from the provider page (it ends with a slash) and check both
documents:

```console
$ ISSUER=https://auth.zeus.balem.fr/application/o/chaos-app/
$ curl -s "${ISSUER}.well-known/openid-configuration" | tr ',' '\n' | grep -E 'jwks_uri|token_endpoint|authorization_endpoint'
$ curl -s "${ISSUER}jwks/"
```

Expected: the discovery document resolves, and **`jwks/` contains a key** —
`{"keys":[{"kty":"RSA",…}]}`. If it prints `{}`, the signing key is not RSA;
fix that before going further, or nothing will authenticate.

## 3. Configure chaos

In `/etc/nixos`, in the chaos service settings:

```nix
  oidc = {
    issuer = "https://auth.zeus.balem.fr/application/o/chaos-app/";
    client_id = "<the client ID from step 1>";
  };
```

Both values are required — chaos treats a half-configured block as "off" rather
than trusting anything.

The client ID is not a secret (public clients have none), so it does not need
agenix. The issuer contains the public domain, which the repo treats as
semi-secret; it is already an envsubst placeholder in this config, so follow
whatever the surrounding settings do.

## 4. Add the two traefik routers

The apps must reach the API without the forward-auth redirect, and must be able
to ask "who are you and how do I sign in?" before they hold a token:

```
router A   PathPrefix(`/api/v1`) && HeaderRegexp(`Authorization`, `^Bearer `)  → chaos, no authentik middleware
router A2  Path(`/api/v1/health`)                                              → chaos, no authentik middleware
router B   everything else (the existing one)                                  → unchanged
```

Give A and A2 a higher priority than B. Keep `chaos-cors` on B — preflight
`OPTIONS` requests carry no `Authorization` header, so they land there and are
answered before the forward-auth sees them.

**Order matters: deploy the chaos update before these routers.** The server
half now requires authentication on every API route, so the bypass is safe —
but only once that server is actually running. In the other order there is a
window where `/api/v1` is reachable with no auth at all.

Since both changes rebuild together here, just make sure the chaos package in
this rebuild is the new one.

## 5. Rebuild and check the server

```console
$ sudo nixos-rebuild switch
```

Then, from anywhere:

```console
$ curl -s https://zeus.balem.fr/api/v1/health
```

Expected: a JSON body containing an `auth.oidc` block with your issuer and
client ID — **not** a redirect to authentik.

```console
$ curl -s -o /dev/null -w '%{http_code}\n' -H 'Authorization: Bearer not.a.token' https://zeus.balem.fr/api/v1/dashboard
```

Expected: **401**. A `302` means router A isn't matching; a `200` means chaos
isn't requiring auth (wrong build) — stop and fix that before installing the
app, because it would mean the API is publicly readable.

## 6. Install the app and sign in

Install the APK from the release (or `just apk` locally). Then:

1. Open the app. Enter the server address if it asks.
2. It should say **"This server is protected by authentik"** with a
   **"Sign in with authentik"** button — not "Cannot reach the chaos server".
3. Tap it: your browser opens authentik's own login page (so passkeys, 2FA and
   an existing browser session all work).
4. Approve. The browser hands control back to the app, which picks up the token
   within a second or two and shows the dashboard.

The app polls for two minutes after the handoff; if you take longer, tap the
button again.

## 7. Confirm the things this was meant to fix

- **Offline:** turn on airplane mode and reopen the app. It shows cached data
  and the offline badge, and does **not** sign you out — even if the access
  token expired meanwhile. Reconnect: still signed in.
- **Across updates:** install a newer APK over the top. Still signed in (the
  tokens live in the app data directory, not webview storage).
- **Identity:** the greeting should show your authentik display name, and the
  account should be the same `tibo` chaos account the browser uses.

## Rollback

Remove the `oidc` block and the two routers, rebuild. The app falls back to
showing the connect screen (it will no longer see an OIDC advertisement), and
the browser path is unaffected — as it is at every step above.

## If sign-in fails

| symptom | cause |
| --- | --- |
| App still says "Cannot reach the chaos server" | `/api/v1/health` isn't reachable unauthenticated — router A2 missing or lower priority than B |
| Sign-in button does nothing | not running in a shell (a plain browser has no way to complete the flow — that's expected; the web UI uses the browser's own session) |
| Browser opens, then "the identity provider rejected the code" | redirect URI mismatch, or PKCE not enabled on a public client |
| Signed in, but every API call 401s | JWKS empty (signing key not RSA), or `issuer`/`client_id` in the chaos config don't match the provider exactly — the `iss` and `aud` claims are checked strictly |
| Browser never returns to the app | the callback scheme isn't registered: reinstall the APK built from this branch (the intent-filter is new) |
