<!-- file: changelog.d/cloudflare-access-is-the-login.md -->
<!-- version: 1.0.0 -->
<!-- guid: 9a4d2f18-3e56-4b0c-8d71-6f2a5c93e084 -->
<!-- last-edited: 2026-08-05 -->

### Added

- **`uaa-control` accepts the Cloudflare Access identity the edge already
  established.** `uaa.jdfalk.com` sits behind a Cloudflare Access application,
  so by the time a request reaches the origin the operator has already
  authenticated against the "Only John" policy. The operator plane threw that
  away and redirected to its own GitHub OAuth flow — against an OAuth app that
  does not exist (`UAA_GITHUB_CLIENT_ID` is empty), leaving the bootstrap token
  as the only way in. Two logins, one of them broken.

  The new `cf_access` module verifies the forwarded `Cf-Access-Jwt-Assertion`
  (or `CF_Authorization` cookie) and exchanges it for the ordinary signed
  session cookie, so nothing downstream has to know how the session began.

  Configured entirely by environment: `UAA_CF_ACCESS_TEAM_DOMAIN`,
  `UAA_CF_ACCESS_AUD`, `UAA_CF_ACCESS_ADMINS` / `_OPERATORS` / `_VIEWERS`, and
  `UAA_CF_ACCESS_DISABLE`.

### Security

- **Only the signed assertion is trusted; the email header never is.** `:15000`
  is reachable directly on the LAN — Cloudflare dials it with
  `noTLSVerify: true` and there is no client-certificate gate — so any host on
  the segment can set any header it likes. Access also forwards
  `Cf-Access-Authenticated-User-Email`, which is an unauthenticated string;
  this code never reads it. Identity comes only from verified claims. Two
  regression tests pin this, one at the header helper and one through the real
  middleware.

- **`aud` and `iss` are both pinned, and a half-configured deployment verifies
  nothing.** `iss` alone would accept a token minted for any *other* Access
  application in the same team (each of which has its own, possibly broader,
  policy); `aud` alone would accept one from an attacker's own Cloudflare team.
  `CfAccessConfig::enabled` requires both, so a missing value disables the login
  method rather than verifying half of it. Note the AUD tag is a 64-hex-char
  value from the Access API, *not* the application's UUID.

- **A cryptographically valid token is not by itself an authorization.** The
  Access policy lives in a dashboard this code cannot read and can be widened
  without any change landing in this repo, so a verified email that appears in
  no allowlist is rejected outright rather than defaulted to Viewer.

- **Escalation on 403 is closed.** The edge identity is only consulted when
  there is no valid session at all. Re-deriving a role on an
  already-authenticated-but-under-privileged request would let a Viewer escalate
  by replaying the same Access token that produced their Viewer session.

- `jsonwebtoken`'s `rust_crypto` feature is load-bearing rather than a
  preference: version 11 resolves its crypto backend at runtime and **panics on
  the first verification** if no backend feature is selected, while compiling
  cleanly either way. Caught by the tests, which is the only place it could have
  been caught before production.
