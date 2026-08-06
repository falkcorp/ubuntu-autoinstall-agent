// file: crates/uaa-control/src/cf_access.rs
// version: 1.0.0
// guid: 5b3e9a41-7c62-4d08-9f15-2a6e8b0d4c73
// last-edited: 2026-08-05

//! Cloudflare Access identity: accept the JWT the edge already minted, instead
//! of making an operator log in a second time.
//!
//! `uaa.jdfalk.com` sits behind a Cloudflare Tunnel with a Cloudflare Access
//! application in front of it (`~/repos/temp/cloudflare-one/HANDOFF.md` §1).
//! By the time a request reaches this process, the edge has ALREADY
//! authenticated the human against the "Only John" policy and attached a signed
//! assertion of who they are. Before this module existed, `uaa-control` threw
//! that away and redirected to its own GitHub OAuth flow — against an OAuth app
//! that does not exist ([`crate::auth::AuthConfig::client_id`] is empty), so the
//! only way in was the bootstrap token. Two logins, one of them broken.
//!
//! # What is and is not trusted
//!
//! **`:15000` is reachable directly on the LAN.** Cloudflare dials the origin
//! with `noTLSVerify: true` and there is no client-certificate gate, so any host
//! on 172.16.2.0/23 can connect and set any header it likes. Therefore:
//!
//! * The ONLY trusted input is the RS256-signed JWT in the
//!   `Cf-Access-Jwt-Assertion` header (or the `CF_Authorization` cookie, which
//!   carries the same token) — and only after its signature, `aud`, `iss`, and
//!   `exp`/`nbf` have all been checked against Cloudflare's published keys.
//! * `Cf-Access-Authenticated-User-Email` — which Access also forwards — is
//!   **never read by this module, and must never be**. It is an unauthenticated
//!   string that any LAN host can set to any value. The identity always comes
//!   out of the verified claims.
//!
//! Pinning BOTH `aud` and `iss` is load-bearing, not belt-and-braces. `iss`
//! alone would accept a token minted for any *other* Access application in the
//! same team (e.g. `media.jdfalk.com`, which has a different, broader policy);
//! `aud` alone would accept a token from an attacker's own Cloudflare team that
//! happened to reuse the tag. [`CfAccessConfig::enabled`] requires both to be
//! configured, so a half-configured deployment disables the path entirely rather
//! than verifying half of it.
//!
//! # This is a second way to mint a session — deliberately, and on the record
//!
//! [`crate::auth`]'s module doc records spec Decision 8's "no login bypass"
//! policy, and the one narrow, disable-able exception already carved out of it
//! (the bootstrap admin token). This module is the second exception, and it
//! follows the same shape on purpose:
//!
//! * It is **off unless explicitly configured** — no team domain or no AUD tag
//!   means the code path does not exist, rather than degrading to "accept
//!   anything" (see [`CfAccessConfig::enabled`]).
//! * It is **disable-able** at runtime via `UAA_CF_ACCESS_DISABLE`.
//! * It grants a role from an **operator-controlled allowlist**
//!   ([`CfAccessConfig::admins`] / [`CfAccessConfig::operators`] /
//!   [`CfAccessConfig::viewers`]), NOT from "any identity Access let through".
//!   The Access policy lives in a Cloudflare dashboard this code cannot read and
//!   that can be widened without any change landing in this repo; an email that
//!   verifies cryptographically but is in none of the lists is rejected outright.
//!
//! Unlike the bootstrap token, this exception is not a stopgap: it is the
//! intended long-term operator login. The bootstrap token should be disabled
//! (`UAA_OPERATOR_DISABLE_BOOTSTRAP_TOKEN`, or the admin API) once this is
//! confirmed working, so that enabling one door closes another rather than
//! leaving both open.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use axum::http::{header, HeaderMap};
use serde::Deserialize;

use crate::auth::Role;

/// The header Cloudflare Access attaches to every proxied request.
pub const CF_JWT_HEADER: &str = "cf-access-jwt-assertion";

/// The cookie Access sets in the browser, carrying the same JWT. Read as a
/// fallback for requests that reach the origin without the header.
pub const CF_JWT_COOKIE: &str = "CF_Authorization";

/// How long a fetched JWKS is reused before being re-fetched. Mirrors
/// [`crate::auth::ROLE_CACHE_TTL`]'s shape: without a cache, Cloudflare becomes a
/// hard synchronous dependency of every single request.
pub const JWKS_CACHE_TTL: Duration = Duration::from_secs(15 * 60);

// ── Config ──────────────────────────────────────────────────────────────────────

/// Cloudflare Access configuration. Every field is operator-supplied; nothing is
/// inferred, and an incomplete config disables the path (see [`Self::enabled`]).
#[derive(Debug, Clone, Default)]
pub struct CfAccessConfig {
    /// `UAA_CF_ACCESS_TEAM_DOMAIN`, e.g. `jdfalk.cloudflareaccess.com`. Both the
    /// expected `iss` and the JWKS URL derive from this.
    pub team_domain: String,
    /// `UAA_CF_ACCESS_AUD` — the Access application's audience tag. This is a
    /// 64-hex-character value from `GET /accounts/{id}/access/apps/{app_id}`
    /// (`.result.aud`), and is NOT the application's UUID. Pinning the UUID by
    /// mistake makes every verification fail for a non-obvious reason.
    pub aud: String,
    /// `UAA_CF_ACCESS_ADMINS` — comma-separated emails granted [`Role::Admin`].
    pub admins: Vec<String>,
    /// `UAA_CF_ACCESS_OPERATORS` — comma-separated emails granted [`Role::Operator`].
    pub operators: Vec<String>,
    /// `UAA_CF_ACCESS_VIEWERS` — comma-separated emails granted [`Role::Viewer`].
    pub viewers: Vec<String>,
    /// `UAA_CF_ACCESS_DISABLE` — set to any non-empty value to turn the whole
    /// path off without unsetting the rest of the configuration.
    pub disabled: bool,
}

impl CfAccessConfig {
    /// Builds config from the environment. Absent variables leave empty values,
    /// which [`Self::enabled`] treats as "not configured".
    pub fn from_env() -> Self {
        Self {
            team_domain: env_trimmed("UAA_CF_ACCESS_TEAM_DOMAIN"),
            aud: env_trimmed("UAA_CF_ACCESS_AUD"),
            admins: env_email_list("UAA_CF_ACCESS_ADMINS"),
            operators: env_email_list("UAA_CF_ACCESS_OPERATORS"),
            viewers: env_email_list("UAA_CF_ACCESS_VIEWERS"),
            disabled: !env_trimmed("UAA_CF_ACCESS_DISABLE").is_empty(),
        }
    }

    /// Whether the Cloudflare Access path is live.
    ///
    /// Requires BOTH `team_domain` and `aud`: a config with only one of them is
    /// a misconfiguration, and the safe reading of a misconfiguration is "this
    /// login method does not exist", never "verify the half we were given".
    pub fn enabled(&self) -> bool {
        !self.disabled && !self.team_domain.is_empty() && !self.aud.is_empty()
    }

    /// The expected `iss` claim.
    pub fn issuer(&self) -> String {
        format!("https://{}", self.team_domain)
    }

    /// Cloudflare's published signing keys for this team.
    pub fn jwks_url(&self) -> String {
        format!("https://{}/cdn-cgi/access/certs", self.team_domain)
    }

    /// Maps a verified email to a role, or `None` if it appears in no allowlist.
    ///
    /// `None` means "reject", not "downgrade to Viewer". A cryptographically
    /// valid token proves Cloudflare authenticated *somebody* against a policy
    /// this code cannot see; it does not establish that this deployment intends
    /// that person to have any access at all. Comparison is ASCII-case-
    /// insensitive because email local parts are not case-sensitive in practice
    /// and identity providers vary in what casing they emit.
    pub fn role_for(&self, email: &str) -> Option<Role> {
        let email = email.trim().to_ascii_lowercase();
        if email.is_empty() {
            return None;
        }
        let listed = |list: &[String]| list.iter().any(|e| e.eq_ignore_ascii_case(&email));
        if listed(&self.admins) {
            Some(Role::Admin)
        } else if listed(&self.operators) {
            Some(Role::Operator)
        } else if listed(&self.viewers) {
            Some(Role::Viewer)
        } else {
            None
        }
    }
}

fn env_trimmed(name: &str) -> String {
    std::env::var(name).unwrap_or_default().trim().to_string()
}

fn env_email_list(name: &str) -> Vec<String> {
    std::env::var(name)
        .unwrap_or_default()
        .split(',')
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty())
        .collect()
}

// ── JWKS ────────────────────────────────────────────────────────────────────────

/// One RSA signing key from Cloudflare's JWKS, reduced to what verification
/// needs. Keys with a non-RSA `kty` are dropped at parse time.
#[derive(Debug, Clone, Deserialize)]
pub struct JwkRsa {
    pub kid: String,
    /// Base64url modulus.
    pub n: String,
    /// Base64url exponent.
    pub e: String,
}

#[derive(Debug, Deserialize)]
struct JwksDoc {
    keys: Vec<JwkEntry>,
}

#[derive(Debug, Deserialize)]
struct JwkEntry {
    kty: String,
    kid: Option<String>,
    n: Option<String>,
    e: Option<String>,
}

/// Parses a JWKS document, keeping only usable RSA keys.
pub fn parse_jwks(body: &str) -> Result<Vec<JwkRsa>> {
    let doc: JwksDoc = serde_json::from_str(body)?;
    Ok(doc
        .keys
        .into_iter()
        .filter(|k| k.kty == "RSA")
        .filter_map(|k| {
            Some(JwkRsa {
                kid: k.kid?,
                n: k.n?,
                e: k.e?,
            })
        })
        .collect())
}

/// Where signing keys come from. A trait so tests can verify real tokens against
/// a locally generated keypair without touching the network — the same seam
/// [`crate::auth::GithubApi`] uses for the same reason.
#[async_trait]
pub trait JwksSource: Send + Sync {
    async fn fetch(&self, url: &str) -> Result<Vec<JwkRsa>>;
}

/// Production [`JwksSource`]: `reqwest` against Cloudflare's certs endpoint.
pub struct HttpJwksSource {
    http: reqwest::Client,
}

impl Default for HttpJwksSource {
    fn default() -> Self {
        Self {
            http: reqwest::Client::new(),
        }
    }
}

#[async_trait]
impl JwksSource for HttpJwksSource {
    async fn fetch(&self, url: &str) -> Result<Vec<JwkRsa>> {
        let body = self
            .http
            .get(url)
            .timeout(Duration::from_secs(10))
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?;
        parse_jwks(&body)
    }
}

// ── Verification ────────────────────────────────────────────────────────────────

/// A verified Cloudflare Access identity. Constructed only after signature,
/// `aud`, `iss`, and expiry have all passed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CfIdentity {
    /// The `email` claim from the verified token.
    pub email: String,
    /// The role this deployment's allowlist grants that email.
    pub role: Role,
}

/// Claims we care about. Cloudflare sends more; the rest is ignored.
#[derive(Debug, Deserialize)]
struct CfClaims {
    #[serde(default)]
    email: String,
}

/// Why a token was rejected. Logged (never the token itself) so
/// `journalctl -u uaa-control` shows which check failed rather than a bare
/// "denied" — a verifier whose failures are indistinguishable is a verifier
/// nobody can debug.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CfReject {
    /// No `Cf-Access-Jwt-Assertion` header and no `CF_Authorization` cookie.
    NoToken,
    /// The JWT header is unparseable or names no `kid`.
    MalformedToken,
    /// The token's `kid` matches no key in the (freshly fetched) JWKS.
    UnknownKid,
    /// Could not obtain Cloudflare's signing keys.
    JwksUnavailable,
    /// Signature, `aud`, `iss`, or `exp`/`nbf` failed. Deliberately one variant:
    /// distinguishing them for a caller would be a validity oracle.
    InvalidToken,
    /// Cryptographically valid, but the email is in no allowlist.
    NotAuthorized(String),
}

impl std::fmt::Display for CfReject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CfReject::NoToken => write!(f, "no Access token on the request"),
            CfReject::MalformedToken => write!(f, "token header unparseable or missing kid"),
            CfReject::UnknownKid => write!(f, "token kid not present in Cloudflare's JWKS"),
            CfReject::JwksUnavailable => write!(f, "could not fetch Cloudflare's signing keys"),
            CfReject::InvalidToken => write!(f, "signature, aud, iss, or expiry check failed"),
            CfReject::NotAuthorized(email) => {
                write!(f, "{email} verified but is in no access allowlist")
            }
        }
    }
}

/// Holds config, the JWKS source, and the key cache.
pub struct CfAccessState {
    config: CfAccessConfig,
    jwks: Box<dyn JwksSource>,
    cache: Mutex<Option<(Vec<JwkRsa>, Instant)>>,
}

impl CfAccessState {
    pub fn new(config: CfAccessConfig, jwks: Box<dyn JwksSource>) -> Self {
        Self {
            config,
            jwks,
            cache: Mutex::new(None),
        }
    }

    /// Convenience constructor for production wiring.
    pub fn from_env() -> Self {
        Self::new(CfAccessConfig::from_env(), Box::new(HttpJwksSource::default()))
    }

    pub fn config(&self) -> &CfAccessConfig {
        &self.config
    }

    pub fn enabled(&self) -> bool {
        self.config.enabled()
    }

    /// Returns cached keys when fresh, otherwise fetches and re-caches.
    ///
    /// On a fetch failure with a stale cache present, the STALE keys are used
    /// rather than failing the request. This is safe and deliberate: JWKS
    /// entries are public signing keys, not secrets, and a stale key can only
    /// ever *verify* a signature Cloudflare actually produced — it cannot admit
    /// a forged one. Failing closed here would instead mean a transient
    /// Cloudflare blip logs every operator out mid-incident.
    async fn keys(&self) -> Option<Vec<JwkRsa>> {
        if let Some((keys, fetched_at)) = self.cache.lock().unwrap().as_ref() {
            if fetched_at.elapsed() < JWKS_CACHE_TTL {
                return Some(keys.clone());
            }
        }
        match self.jwks.fetch(&self.config.jwks_url()).await {
            Ok(keys) => {
                *self.cache.lock().unwrap() = Some((keys.clone(), Instant::now()));
                Some(keys)
            }
            Err(err) => {
                tracing::warn!(%err, "cf-access: JWKS fetch failed");
                self.cache
                    .lock()
                    .unwrap()
                    .as_ref()
                    .map(|(keys, _)| keys.clone())
            }
        }
    }

    /// Verifies `token` and resolves it to an authorized identity.
    ///
    /// Every check is mandatory: RS256 only (the algorithm is pinned, so an
    /// `alg: none` or HMAC-substitution token cannot be presented), a `kid` that
    /// names a key Cloudflare actually published, the configured `aud`, the
    /// configured `iss`, and expiry.
    pub async fn verify(&self, token: &str) -> std::result::Result<CfIdentity, CfReject> {
        if !self.enabled() {
            return Err(CfReject::NoToken);
        }
        let kid = kid_of(token).ok_or(CfReject::MalformedToken)?;
        let keys = self.keys().await.ok_or(CfReject::JwksUnavailable)?;
        let key = keys
            .iter()
            .find(|k| k.kid == kid)
            .ok_or(CfReject::UnknownKid)?;

        let claims = decode_rs256(token, key, &self.config.aud, &self.config.issuer())
            .map_err(|_| CfReject::InvalidToken)?;

        match self.config.role_for(&claims.email) {
            Some(role) => Ok(CfIdentity {
                email: claims.email,
                role,
            }),
            None => Err(CfReject::NotAuthorized(claims.email)),
        }
    }

    /// Pulls the token off a request and verifies it. `Cf-Access-Jwt-Assertion`
    /// first, then the `CF_Authorization` cookie.
    ///
    /// Note what is NOT consulted: `Cf-Access-Authenticated-User-Email`. See the
    /// module doc — that header is an unauthenticated string on a LAN-reachable
    /// port.
    pub async fn identify(
        &self,
        headers: &HeaderMap,
    ) -> std::result::Result<CfIdentity, CfReject> {
        let token = token_from_headers(headers).ok_or(CfReject::NoToken)?;
        self.verify(&token).await
    }
}

/// Extracts the Access JWT from a request's headers.
pub fn token_from_headers(headers: &HeaderMap) -> Option<String> {
    if let Some(v) = headers.get(CF_JWT_HEADER).and_then(|v| v.to_str().ok()) {
        let v = v.trim();
        if !v.is_empty() {
            return Some(v.to_string());
        }
    }
    let raw = headers.get(header::COOKIE)?.to_str().ok()?;
    raw.split(';').map(str::trim).find_map(|kv| {
        kv.strip_prefix(CF_JWT_COOKIE)
            .and_then(|rest| rest.strip_prefix('='))
            .filter(|v| !v.is_empty())
            .map(str::to_string)
    })
}

/// Reads the `kid` out of a JWT header segment without validating anything.
fn kid_of(token: &str) -> Option<String> {
    let header = jsonwebtoken::decode_header(token).ok()?;
    header.kid
}

/// The actual RS256 decode, with `aud` and `iss` pinned.
///
/// `jsonwebtoken`'s `Validation::new` validates `exp` but leaves the audience
/// and issuer sets EMPTY, and an empty set means the claim is not checked at
/// all. Both are set explicitly here; dropping either silently widens what this
/// origin accepts to every Access app in the team (or every team).
fn decode_rs256(token: &str, key: &JwkRsa, aud: &str, iss: &str) -> Result<CfClaims> {
    use jsonwebtoken::{decode, Algorithm, DecodingKey, Validation};

    let decoding = DecodingKey::from_rsa_components(&key.n, &key.e)
        .map_err(|e| anyhow!("bad RSA components in JWKS: {e}"))?;
    let mut validation = Validation::new(Algorithm::RS256);
    validation.set_audience(&[aud]);
    validation.set_issuer(&[iss]);
    validation.validate_exp = true;
    validation.validate_nbf = true;

    let data = decode::<CfClaims>(token, &decoding, &validation)
        .map_err(|e| anyhow!("token rejected: {e}"))?;
    Ok(data.claims)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> CfAccessConfig {
        CfAccessConfig {
            team_domain: "jdfalk.cloudflareaccess.com".to_string(),
            aud: "c5d9a7f2360d595c94a0266a1c829a80bbe29984da7b06c6e53bf6936e02eb48".to_string(),
            admins: vec!["johnathan.falk@gmail.com".to_string()],
            operators: vec![],
            viewers: vec![],
            disabled: false,
        }
    }

    #[test]
    fn enabled_requires_both_team_domain_and_aud() {
        assert!(cfg().enabled());

        let mut no_aud = cfg();
        no_aud.aud = String::new();
        assert!(
            !no_aud.enabled(),
            "a config without an AUD tag must disable the path entirely — verifying \
             iss alone would accept tokens minted for any other Access app in the team"
        );

        let mut no_domain = cfg();
        no_domain.team_domain = String::new();
        assert!(!no_domain.enabled());

        let mut off = cfg();
        off.disabled = true;
        assert!(!off.enabled(), "UAA_CF_ACCESS_DISABLE must turn the path off");
    }

    #[test]
    fn issuer_and_jwks_url_derive_from_the_team_domain() {
        assert_eq!(cfg().issuer(), "https://jdfalk.cloudflareaccess.com");
        assert_eq!(
            cfg().jwks_url(),
            "https://jdfalk.cloudflareaccess.com/cdn-cgi/access/certs"
        );
    }

    #[test]
    fn role_comes_from_the_allowlist_not_from_mere_validity() {
        let c = cfg();
        assert_eq!(c.role_for("johnathan.falk@gmail.com"), Some(Role::Admin));
        // Case differences in what an IdP emits must not change the decision.
        assert_eq!(c.role_for("Johnathan.Falk@Gmail.com"), Some(Role::Admin));
        // A verified stranger gets nothing. The Access policy lives in a
        // dashboard this code cannot read and can be widened without any
        // change landing here.
        assert_eq!(c.role_for("someone.else@example.com"), None);
        assert_eq!(c.role_for(""), None);
    }

    #[test]
    fn allowlists_are_ranked_admin_over_operator_over_viewer() {
        let c = CfAccessConfig {
            admins: vec!["a@x.com".into()],
            operators: vec!["o@x.com".into()],
            viewers: vec!["v@x.com".into()],
            ..cfg()
        };
        assert_eq!(c.role_for("a@x.com"), Some(Role::Admin));
        assert_eq!(c.role_for("o@x.com"), Some(Role::Operator));
        assert_eq!(c.role_for("v@x.com"), Some(Role::Viewer));
    }

    #[test]
    fn token_is_read_from_the_header_then_the_cookie() {
        let mut h = HeaderMap::new();
        assert_eq!(token_from_headers(&h), None);

        h.insert(header::COOKIE, "CF_Authorization=from-cookie".parse().unwrap());
        assert_eq!(token_from_headers(&h), Some("from-cookie".to_string()));

        h.insert(CF_JWT_HEADER, "from-header".parse().unwrap());
        assert_eq!(
            token_from_headers(&h),
            Some("from-header".to_string()),
            "the header wins when both are present"
        );
    }

    /// The regression test that matters most. `Cf-Access-Authenticated-User-Email`
    /// is forwarded by Access and is trivially forgeable by any LAN host, because
    /// `:15000` is reachable directly. It must never be a source of identity.
    #[test]
    fn the_forgeable_email_header_is_never_a_token_source() {
        let mut h = HeaderMap::new();
        h.insert(
            "cf-access-authenticated-user-email",
            "attacker@evil.example".parse().unwrap(),
        );
        assert_eq!(
            token_from_headers(&h),
            None,
            "an unsigned email header must yield no token — identity comes only \
             from the verified JWT claims"
        );
    }

    #[test]
    fn jwks_parsing_keeps_rsa_keys_and_drops_the_rest() {
        let body = r#"{"keys":[
            {"kty":"RSA","kid":"k1","n":"AQAB","e":"AQAB"},
            {"kty":"oct","kid":"sym","k":"nope"},
            {"kty":"RSA","kid":"k2","n":"AQAB","e":"AQAB"},
            {"kty":"RSA","n":"AQAB","e":"AQAB"}
        ]}"#;
        let keys = parse_jwks(body).expect("parses");
        let kids: Vec<_> = keys.iter().map(|k| k.kid.as_str()).collect();
        assert_eq!(
            kids,
            vec!["k1", "k2"],
            "non-RSA keys and keys with no kid are unusable and must be dropped"
        );
    }

    #[test]
    fn a_garbage_token_has_no_kid() {
        assert_eq!(kid_of("not-a-jwt"), None);
        assert_eq!(kid_of(""), None);
    }

    // ── End-to-end crypto: real RS256 tokens against a local keypair ────────────
    //
    // The tests above check the plumbing. These check the actual security
    // property, because a verifier that has only ever been shown VALID tokens
    // is indistinguishable from a function that returns `Ok`. Each of the
    // mandatory checks — signature, `aud`, `iss`, `exp`, `kid` — gets a token
    // that is correct in every respect except that one.

    /// Test-only 2048-bit RSA key. Generated for this test file, used nowhere
    /// else, and never valid for anything: Cloudflare signs with its own keys,
    /// so possession of this one grants nothing against a real deployment.
    const TEST_KEY_PEM: &str = include_str!("../tests/fixtures/cf_access_test_key.pem");
    const TEST_KEY_N: &str = "uZkVL-EGKBgcxQCk8rrW7jgWQ7y32NxjP1oZ4v-SdllOWL9ypIDg1HqVf3WDURNiwcQWAR7epV2BG0KxJBen_XjD4MOtzwYfCudOWvznAs7MT3YewXFYOrK_2KoQZrmulLwOVgwMBfjj5HWgWLoK68cKE7jfno0NASQWtz5YWH6xQz5EVCbftf-MkLKC4vw-kNoJOutlPrpA68FwQ3ZlAqvoYYawxEKYt8U1WlZw3cHoIc5zWfKDu7xj2DLNSqJUirxxyYncQzoapnY7YFU1oYF_0wcTxt-xdQYzH3mIsEpo_BcKuXjyO6qk6mn5iPshAgbBEkoh2AqdM_HH6Hc-uQ";
    const TEST_KEY_E: &str = "AQAB";
    const TEST_KID: &str = "test-kid-1";

    fn test_jwk() -> JwkRsa {
        JwkRsa {
            kid: TEST_KID.to_string(),
            n: TEST_KEY_N.to_string(),
            e: TEST_KEY_E.to_string(),
        }
    }

    /// A [`JwksSource`] that serves the local test key without any network.
    struct StaticJwks(Vec<JwkRsa>);

    #[async_trait]
    impl JwksSource for StaticJwks {
        async fn fetch(&self, _url: &str) -> Result<Vec<JwkRsa>> {
            Ok(self.0.clone())
        }
    }

    /// A [`JwksSource`] that always fails, for the unavailable-keys path.
    struct FailingJwks;

    #[async_trait]
    impl JwksSource for FailingJwks {
        async fn fetch(&self, _url: &str) -> Result<Vec<JwkRsa>> {
            Err(anyhow!("simulated JWKS outage"))
        }
    }

    #[derive(serde::Serialize)]
    struct TestClaims {
        email: String,
        aud: String,
        iss: String,
        exp: u64,
        nbf: u64,
    }

    fn now() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
    }

    /// Mints a real RS256 token signed by the test key.
    fn mint(email: &str, aud: &str, iss: &str, exp: u64, kid: &str) -> String {
        use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some(kid.to_string());
        let claims = TestClaims {
            email: email.to_string(),
            aud: aud.to_string(),
            iss: iss.to_string(),
            exp,
            nbf: now().saturating_sub(60),
        };
        let key = EncodingKey::from_rsa_pem(TEST_KEY_PEM.as_bytes()).expect("test key parses");
        encode(&header, &claims, &key).expect("test token encodes")
    }

    fn state() -> CfAccessState {
        CfAccessState::new(cfg(), Box::new(StaticJwks(vec![test_jwk()])))
    }

    /// A well-formed token for the configured app, signed by the expected key,
    /// naming an allowlisted email.
    fn good_token() -> String {
        let c = cfg();
        mint(
            "johnathan.falk@gmail.com",
            &c.aud,
            &c.issuer(),
            now() + 3600,
            TEST_KID,
        )
    }

    #[tokio::test]
    async fn a_valid_token_resolves_to_the_allowlisted_role() {
        let identity = state().verify(&good_token()).await.expect("must verify");
        assert_eq!(identity.email, "johnathan.falk@gmail.com");
        assert_eq!(identity.role, Role::Admin);
    }

    #[tokio::test]
    async fn a_token_for_another_access_app_is_rejected() {
        // Same team, same signing key, same everything — different `aud`. This is
        // the token `media.jdfalk.com` would mint, and its Access policy is not
        // this one's.
        let c = cfg();
        let token = mint(
            "johnathan.falk@gmail.com",
            "0000000000000000000000000000000000000000000000000000000000000000",
            &c.issuer(),
            now() + 3600,
            TEST_KID,
        );
        assert_eq!(
            state().verify(&token).await.unwrap_err(),
            CfReject::InvalidToken,
            "aud must be pinned — otherwise any Access app in the team opens this one"
        );
    }

    #[tokio::test]
    async fn a_token_from_another_team_is_rejected() {
        let c = cfg();
        let token = mint(
            "johnathan.falk@gmail.com",
            &c.aud,
            "https://attacker.cloudflareaccess.com",
            now() + 3600,
            TEST_KID,
        );
        assert_eq!(
            state().verify(&token).await.unwrap_err(),
            CfReject::InvalidToken,
            "iss must be pinned to our team domain"
        );
    }

    #[tokio::test]
    async fn an_expired_token_is_rejected() {
        let c = cfg();
        let token = mint(
            "johnathan.falk@gmail.com",
            &c.aud,
            &c.issuer(),
            now() - 3600,
            TEST_KID,
        );
        assert_eq!(
            state().verify(&token).await.unwrap_err(),
            CfReject::InvalidToken
        );
    }

    #[tokio::test]
    async fn a_tampered_signature_is_rejected() {
        let token = good_token();
        // Corrupt the signature segment only; header and payload stay valid, so
        // this isolates the signature check from the claim checks.
        let (rest, sig) = token.rsplit_once('.').unwrap();
        let flipped: String = sig
            .chars()
            .map(|ch| if ch == 'A' { 'B' } else { 'A' })
            .collect();
        assert_eq!(
            state()
                .verify(&format!("{rest}.{flipped}"))
                .await
                .unwrap_err(),
            CfReject::InvalidToken,
            "an unsigned or re-signed token must never verify"
        );
    }

    #[tokio::test]
    async fn a_token_naming_an_unpublished_kid_is_rejected() {
        let c = cfg();
        let token = mint(
            "johnathan.falk@gmail.com",
            &c.aud,
            &c.issuer(),
            now() + 3600,
            "some-other-kid",
        );
        assert_eq!(
            state().verify(&token).await.unwrap_err(),
            CfReject::UnknownKid
        );
    }

    #[tokio::test]
    async fn a_cryptographically_valid_stranger_gets_nothing() {
        let c = cfg();
        let token = mint(
            "stranger@example.com",
            &c.aud,
            &c.issuer(),
            now() + 3600,
            TEST_KID,
        );
        assert_eq!(
            state().verify(&token).await.unwrap_err(),
            CfReject::NotAuthorized("stranger@example.com".to_string()),
            "Access having let someone through is not this deployment's decision \
             to grant them a role"
        );
    }

    #[tokio::test]
    async fn a_disabled_config_verifies_nothing() {
        let mut c = cfg();
        c.disabled = true;
        let s = CfAccessState::new(c, Box::new(StaticJwks(vec![test_jwk()])));
        assert_eq!(s.verify(&good_token()).await.unwrap_err(), CfReject::NoToken);
    }

    #[tokio::test]
    async fn no_keys_and_no_cache_fails_closed() {
        let s = CfAccessState::new(cfg(), Box::new(FailingJwks));
        assert_eq!(
            s.verify(&good_token()).await.unwrap_err(),
            CfReject::JwksUnavailable,
            "with no keys and nothing cached there is no way to verify — deny"
        );
    }

    #[tokio::test]
    async fn identify_reads_the_header_and_verifies_it() {
        let mut h = HeaderMap::new();
        h.insert(CF_JWT_HEADER, good_token().parse().unwrap());
        assert_eq!(state().identify(&h).await.unwrap().role, Role::Admin);
    }

    /// The end-to-end statement of the module's central rule, on the real
    /// verifier rather than just the header helper.
    #[tokio::test]
    async fn a_forged_email_header_alone_authenticates_nobody() {
        let mut h = HeaderMap::new();
        h.insert(
            "cf-access-authenticated-user-email",
            "johnathan.falk@gmail.com".parse().unwrap(),
        );
        assert_eq!(
            state().identify(&h).await.unwrap_err(),
            CfReject::NoToken,
            ":15000 is LAN-reachable, so any host can set this header. Only the \
             signed assertion counts."
        );
    }

    // ── Through the actual middleware ──────────────────────────────────────────
    //
    // Everything above tests the verifier in isolation. These drive a real
    // `axum` router through `auth::require_role`, because the property the user
    // actually cares about — "Cloudflare already logged me in, stop asking
    // again" — lives in the wiring, not the verifier. A correct verifier that
    // nothing calls produces the exact same broken login page.

    use crate::auth::{require_role, AuthConfig, AuthState, GithubApi, RealGithubApi};
    use axum::body::Body;
    use axum::http::Request;
    use axum::routing::get;
    use axum::Router;
    use std::sync::Arc;
    use tower::ServiceExt as _;

    fn guarded_router(dir: &std::path::Path) -> Router {
        let auth_config = AuthConfig {
            client_id: String::new(),
            client_secret: String::new(),
            org: "falkcorp".to_string(),
            admin_team: "uaa-admins".to_string(),
            operator_team: "uaa-operators".to_string(),
            state_dir: dir.to_path_buf(),
        };
        let hmac_key = crate::auth::load_or_create_hmac_key(dir).unwrap();
        // No GitHub app is configured in this deployment; the OAuth backend is
        // present but unreachable, which is precisely the situation Cloudflare
        // Access has to work in.
        let github: Arc<dyn GithubApi> =
            Arc::new(RealGithubApi::new(String::new(), String::new(), String::new()));
        let auth_state = AuthState::new(auth_config, github, hmac_key);

        require_role(
            Router::new().route("/api/thing", get(|| async { "ok" })),
            Role::Viewer,
        )
        .layer(axum::Extension(auth_state))
        .layer(axum::Extension(Arc::new(state())))
    }

    #[tokio::test]
    async fn a_valid_access_token_gets_in_with_no_prior_session() {
        let dir = tempfile::tempdir().unwrap();
        let resp = guarded_router(dir.path())
            .oneshot(
                Request::builder()
                    .uri("/api/thing")
                    .header(CF_JWT_HEADER, good_token())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            200,
            "an operator Cloudflare already authenticated must not be asked to log in again"
        );
        let set_cookie = resp
            .headers()
            .get(axum::http::header::SET_COOKIE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default();
        assert!(
            set_cookie.contains("uaa_session="),
            "the edge identity must be exchanged for the ordinary signed session \
             cookie, so later requests skip the JWKS round trip: got {set_cookie:?}"
        );
    }

    #[tokio::test]
    async fn the_forgeable_email_header_does_not_get_past_the_middleware() {
        let dir = tempfile::tempdir().unwrap();
        let resp = guarded_router(dir.path())
            .oneshot(
                Request::builder()
                    .uri("/api/thing")
                    .header("cf-access-authenticated-user-email", "johnathan.falk@gmail.com")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            401,
            "any LAN host can set this header; it must buy exactly nothing"
        );
    }

    #[tokio::test]
    async fn a_token_for_a_different_access_app_does_not_get_past_the_middleware() {
        let dir = tempfile::tempdir().unwrap();
        let c = cfg();
        let wrong_aud = mint(
            "johnathan.falk@gmail.com",
            "0000000000000000000000000000000000000000000000000000000000000000",
            &c.issuer(),
            now() + 3600,
            TEST_KID,
        );
        let resp = guarded_router(dir.path())
            .oneshot(
                Request::builder()
                    .uri("/api/thing")
                    .header(CF_JWT_HEADER, wrong_aud)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 401);
    }
}
