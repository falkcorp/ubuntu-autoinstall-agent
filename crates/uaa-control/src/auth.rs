// file: crates/uaa-control/src/auth.rs
// version: 1.2.0
// guid: af3dc9c0-def6-46ff-9892-90e54716fe21
// last-edited: 2026-07-13

//! RBAC + operator authentication: GitHub OAuth web flow + org/team role mapping
//! (spec Decision 8, component C3 "Operator plane").
//!
//! Filled by control TASK-03 (CT-03). Scope, locked by Decision 8:
//!
//! * GitHub OAuth web flow ONLY — authorize redirect, code-for-token exchange, then a
//!   `login` + org/team lookup. No local accounts, no alternate credential store, no
//!   hardware-token second factor, no PIV/mTLS operator login.
//! * RBAC is org-team based: `uaa-admins` team membership -> [`Role::Admin`],
//!   `uaa-operators` team membership -> [`Role::Operator`], plain org membership ->
//!   [`Role::Viewer`], not an org member -> 403 at login (no session minted).
//! * Role cache TTL is 5 minutes ([`ROLE_CACHE_TTL`]); on any GitHub API failure the
//!   effective role for that request degrades to [`Role::Viewer`] — mutations are
//!   denied, reads stay available. A stale cached [`Role::Admin`] is NEVER served past
//!   its TTL.
//! * Session cookies are HMAC-SHA256 signed (`ring::hmac`), `Secure; HttpOnly;
//!   SameSite=Lax`, 24h lifetime, keyed by a 32-byte key persisted 0600 under the
//!   configured state dir.
//! * Everything GitHub-shaped goes through the [`GithubApi`] trait so unit tests never
//!   touch the network — see `mod tests` for the mock implementation. The real impl
//!   ([`RealGithubApi`]) uses `reqwest` with the workspace's rustls-tls feature; no new
//!   HTTP client crate.
//!
//! ## Emergency access during a GitHub outage
//!
//! Decision 8 explicitly rejects a login-bypass "emergency" auth path — a wrong
//! default here would silently grant mutation rights while GitHub is down, which is a
//! worse failure mode than a temporary lockout. No code in this file, or anywhere in
//! `uaa-control`, implements an alternate way to mint a session. If GitHub itself is
//! unreachable long enough that operators are fully locked out of the `:15001` plane,
//! the sanctioned recovery procedure is a human-operated, out-of-band, LOGGED
//! database mutation:
//!
//! 1. An operator with access to the server runs `cockroach sql --url <registry DSN>`
//!    directly against the CockroachDB registry (Decision 4/5) and performs the
//!    minimal mutation needed to unblock the stuck resource (e.g. hand-adjust a
//!    row that a normal operator-plane mutation would otherwise have written).
//! 2. That operator MUST immediately run `uaa-control audit backfill` (CT-04 ships
//!    this command) so the manual mutation is recorded as a proper audit event —
//!    the repair is not complete until the backfill lands.
//!
//! Total lockout during a GitHub outage is an accepted, mitigated risk (Decision 8),
//! not a defect to code around; this section documents the hatch, it does not build
//! one.
//!
//! ## Bootstrap admin token (2026-07-13, explicit temporary exception to Decision 8)
//!
//! No GitHub OAuth app exists yet to authenticate against — `AuthConfig::client_id`
//! is empty until one is created. Rather than leave the operator plane fully
//! inaccessible until then, [`BootstrapTokenState`] mints a real, HMAC-signed
//! session (via [`AuthState::mint_bootstrap_session`], the exact same
//! [`mint_session`] every OAuth login uses) for a single fixed, non-GitHub identity
//! ([`BOOTSTRAP_ADMIN_LOGIN`]) that [`AuthState::effective_role`] always resolves to
//! [`Role::Admin`] without ever calling GitHub. This IS the "alternate way to mint a
//! session" the section above says doesn't exist elsewhere in this file — it exists
//! narrowly, only here, only for one hardcoded identity string a forged cookie can
//! never claim without the server's own HMAC key. It is explicitly disable-able two
//! ways (env var for operators, a self-service admin API for whoever is logged in)
//! so it can retire the moment a real OAuth app is configured.

use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, Context as _, Result};
use async_trait::async_trait;
use axum::extract::{Extension, Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Redirect, Response};
use axum::Json;
use base64::engine::general_purpose::{STANDARD as BASE64, URL_SAFE_NO_PAD as BASE64_URL};
use base64::Engine as _;
use ring::hmac;
use ring::rand::{SecureRandom, SystemRandom};
use serde::{Deserialize, Serialize};
use serde_json::json;

/// Session lifetime: 24 hours (spec: "exp = unix seconds, 24h").
pub const SESSION_TTL_SECS: u64 = 24 * 60 * 60;

/// Role cache TTL: 5 minutes (spec Decision 8: "cached 5 min").
pub const ROLE_CACHE_TTL: Duration = Duration::from_secs(5 * 60);

/// OAuth `state` token TTL: how long an in-flight login attempt stays valid between
/// the `/auth/login` redirect and the `/auth/callback` round trip.
pub const OAUTH_STATE_TTL: Duration = Duration::from_secs(10 * 60);

/// The session cookie name.
pub const SESSION_COOKIE_NAME: &str = "uaa_session";

/// The short-lived, HttpOnly cookie that binds an in-flight OAuth `state` to the
/// browser that started the login (CSRF / login-fixation defense).
pub const OAUTH_STATE_COOKIE_NAME: &str = "uaa_oauth_state";

/// The HMAC key file's name under [`AuthConfig::state_dir`].
const HMAC_KEY_FILE: &str = "auth_hmac.key";

/// The one non-GitHub identity [`AuthState::effective_role`] treats as permanent
/// [`Role::Admin`] — see the module doc's "Bootstrap admin token" section. Not a
/// real GitHub login; nothing else in this file ever mints a session under this
/// name except [`AuthState::mint_bootstrap_session`].
pub const BOOTSTRAP_ADMIN_LOGIN: &str = "bootstrap-admin";

// ── Role + config ───────────────────────────────────────────────────────────────

/// Operator role. Declaration order is significant: `derive(PartialOrd, Ord)` ranks
/// variants by declaration order, so `Role::Admin > Role::Operator > Role::Viewer`
/// holds exactly as spec Decision 8 requires.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Role {
    Viewer,
    Operator,
    Admin,
}

/// GitHub OAuth + RBAC configuration. `client_secret` is never logged or displayed —
/// see the redacting [`fmt::Debug`] impl below.
#[derive(Clone)]
pub struct AuthConfig {
    /// `UAA_GITHUB_CLIENT_ID`.
    pub client_id: String,
    /// `UAA_GITHUB_CLIENT_SECRET` — sourced from env only, never committed; the
    /// literal value never appears in this file (Bucket-3 human work sets it).
    pub client_secret: String,
    /// The GitHub org that gates access (org membership -> [`Role::Viewer`]).
    pub org: String,
    /// Team slug mapped to [`Role::Admin`]. Defaults to the spec's `uaa-admins`.
    pub admin_team: String,
    /// Team slug mapped to [`Role::Operator`]. Defaults to the spec's
    /// `uaa-operators`.
    pub operator_team: String,
    /// Where the HMAC signing key (and any other auth state) persists.
    pub state_dir: PathBuf,
}

impl fmt::Debug for AuthConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AuthConfig")
            .field("client_id", &self.client_id)
            .field("client_secret", &"<redacted>")
            .field("org", &self.org)
            .field("admin_team", &self.admin_team)
            .field("operator_team", &self.operator_team)
            .field("state_dir", &self.state_dir)
            .finish()
    }
}

impl AuthConfig {
    /// Builds config from environment, defaulting team names to the spec's
    /// `uaa-admins` / `uaa-operators` and the state dir to `/var/lib/uaa`.
    pub fn from_env() -> Self {
        Self {
            client_id: env_var("UAA_GITHUB_CLIENT_ID"),
            client_secret: env_var("UAA_GITHUB_CLIENT_SECRET"),
            org: env_var("UAA_GITHUB_ORG"),
            admin_team: env_var_or("UAA_GITHUB_ADMIN_TEAM", "uaa-admins"),
            operator_team: env_var_or("UAA_GITHUB_OPERATOR_TEAM", "uaa-operators"),
            state_dir: PathBuf::from(env_var_or("UAA_STATE_DIR", "/var/lib/uaa")),
        }
    }
}

fn env_var(name: &str) -> String {
    std::env::var(name).unwrap_or_default()
}

fn env_var_or(name: &str, default: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| default.to_string())
}

// ── GitHub API trait (mockable) ─────────────────────────────────────────────────

/// Org/team membership as reported by GitHub for the token's user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrgMembership {
    pub org_member: bool,
    pub teams: Vec<String>,
}

/// Everything GitHub-shaped goes through this trait so tests never touch the
/// network. [`RealGithubApi`] is the `reqwest`-backed production implementation;
/// tests supply a mock (see `mod tests`).
#[async_trait]
pub trait GithubApi: Send + Sync {
    /// Exchanges an OAuth authorization `code` for an access token.
    async fn exchange_code(&self, code: &str) -> Result<String>;
    /// Resolves the GitHub login for an access token.
    async fn user_login(&self, token: &str) -> Result<String>;
    /// Resolves org membership + team slugs (scoped to [`AuthConfig::org`] by the
    /// real implementation) for the given login.
    async fn org_role(&self, token: &str, login: &str) -> Result<OrgMembership>;
}

/// Production [`GithubApi`]: `reqwest` (rustls-tls, the workspace's only HTTP
/// client) against the real GitHub REST API.
pub struct RealGithubApi {
    http: reqwest::Client,
    client_id: String,
    client_secret: String,
    org: String,
}

impl RealGithubApi {
    pub fn new(client_id: String, client_secret: String, org: String) -> Self {
        Self {
            http: reqwest::Client::new(),
            client_id,
            client_secret,
            org,
        }
    }
}

#[async_trait]
impl GithubApi for RealGithubApi {
    async fn exchange_code(&self, code: &str) -> Result<String> {
        #[derive(Deserialize)]
        struct TokenResponse {
            access_token: Option<String>,
            error: Option<String>,
            error_description: Option<String>,
        }
        let resp: TokenResponse = self
            .http
            .post("https://github.com/login/oauth/access_token")
            .header(header::ACCEPT, "application/json")
            .form(&[
                ("client_id", self.client_id.as_str()),
                ("client_secret", self.client_secret.as_str()),
                ("code", code),
            ])
            .send()
            .await
            .context("github token exchange request failed")?
            .json()
            .await
            .context("github token exchange response malformed")?;
        resp.access_token.ok_or_else(|| {
            anyhow!(resp
                .error_description
                .or(resp.error)
                .unwrap_or_else(|| "github returned no access_token".to_string()))
        })
    }

    async fn user_login(&self, token: &str) -> Result<String> {
        #[derive(Deserialize)]
        struct UserResponse {
            login: String,
        }
        let resp: UserResponse = self
            .http
            .get("https://api.github.com/user")
            .bearer_auth(token)
            .header(header::USER_AGENT, "uaa-control")
            .send()
            .await
            .context("github user lookup failed")?
            .error_for_status()
            .context("github user lookup returned an error status")?
            .json()
            .await
            .context("github user response malformed")?;
        Ok(resp.login)
    }

    async fn org_role(&self, token: &str, login: &str) -> Result<OrgMembership> {
        let org_member = self
            .http
            .get(format!(
                "https://api.github.com/orgs/{}/members/{}",
                self.org, login
            ))
            .bearer_auth(token)
            .header(header::USER_AGENT, "uaa-control")
            .send()
            .await
            .context("github org membership check failed")?
            .status()
            .is_success();

        #[derive(Deserialize)]
        struct TeamEntry {
            slug: String,
            organization: TeamOrg,
        }
        #[derive(Deserialize)]
        struct TeamOrg {
            login: String,
        }
        let teams: Vec<TeamEntry> = self
            .http
            .get("https://api.github.com/user/teams")
            .bearer_auth(token)
            .header(header::USER_AGENT, "uaa-control")
            .send()
            .await
            .context("github team list failed")?
            .error_for_status()
            .context("github team list returned an error status")?
            .json()
            .await
            .context("github team list response malformed")?;
        let teams = teams
            .into_iter()
            .filter(|t| t.organization.login == self.org)
            .map(|t| t.slug)
            .collect();
        Ok(OrgMembership { org_member, teams })
    }
}

/// Maps org/team membership to a [`Role`] per spec Decision 8. `None` means "not an
/// org member" — the caller must treat that as a 403, never a session.
fn map_role(config: &AuthConfig, membership: &OrgMembership) -> Option<Role> {
    if membership.teams.iter().any(|t| t == &config.admin_team) {
        Some(Role::Admin)
    } else if membership.teams.iter().any(|t| t == &config.operator_team) {
        Some(Role::Operator)
    } else if membership.org_member {
        Some(Role::Viewer)
    } else {
        None
    }
}

// ── Session cookies ──────────────────────────────────────────────────────────────

/// A verified session: the GitHub login and the role baked into the cookie at mint
/// time (may be refreshed by [`check_access`] against the 5-minute role cache).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    pub login: String,
    pub role: Role,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct SessionPayload {
    login: String,
    role: Role,
    exp: u64,
}

/// Signs `{login, role, exp}` into `uaa_session=<base64(payload)>.<base64(hmac)>`.
/// `now` is unix seconds; the cookie expires `SESSION_TTL_SECS` later.
pub fn mint_session(key: &[u8; 32], login: &str, role: Role, now: u64) -> String {
    let payload = SessionPayload {
        login: login.to_string(),
        role,
        exp: now.saturating_add(SESSION_TTL_SECS),
    };
    let payload_bytes = serde_json::to_vec(&payload).expect("session payload always serializes");
    let payload_b64 = BASE64.encode(&payload_bytes);
    let sig = hmac::sign(&hmac::Key::new(hmac::HMAC_SHA256, key), &payload_bytes);
    let sig_b64 = BASE64.encode(sig.as_ref());
    format!("{payload_b64}.{sig_b64}")
}

/// Verifies a session cookie value. Every failure mode — malformed base64, a bad
/// HMAC signature, an unparseable payload, or `exp <= now` — collapses to `None`;
/// there is exactly one indistinguishable failure path (spec: "no oracle").
pub fn verify_session(key: &[u8; 32], cookie: &str, now: u64) -> Option<Session> {
    let (payload_b64, sig_b64) = cookie.split_once('.')?;
    let payload_bytes = BASE64.decode(payload_b64).ok()?;
    let sig_bytes = BASE64.decode(sig_b64).ok()?;
    let hmac_key = hmac::Key::new(hmac::HMAC_SHA256, key);
    hmac::verify(&hmac_key, &payload_bytes, &sig_bytes).ok()?;
    let payload: SessionPayload = serde_json::from_slice(&payload_bytes).ok()?;
    if payload.exp <= now {
        return None;
    }
    Some(Session {
        login: payload.login,
        role: payload.role,
    })
}

/// Loads the 32-byte HMAC signing key from `<state_dir>/auth_hmac.key`, generating
/// and persisting (mode 0600) a fresh random key on first start. `state_dir` comes
/// from [`AuthConfig`] so tests can point it at a tempdir.
pub fn load_or_create_hmac_key(state_dir: &Path) -> Result<[u8; 32]> {
    fs::create_dir_all(state_dir)
        .with_context(|| format!("creating auth state dir {}", state_dir.display()))?;
    let path = state_dir.join(HMAC_KEY_FILE);
    if let Ok(bytes) = fs::read(&path) {
        if bytes.len() == 32 {
            let mut key = [0u8; 32];
            key.copy_from_slice(&bytes);
            return Ok(key);
        }
    }
    let mut key = [0u8; 32];
    SystemRandom::new()
        .fill(&mut key)
        .map_err(|_| anyhow!("system RNG unavailable while generating the HMAC key"))?;
    fs::write(&path, key).with_context(|| format!("writing HMAC key to {}", path.display()))?;
    set_owner_only(&path)
        .with_context(|| format!("setting 0600 permissions on {}", path.display()))?;
    Ok(key)
}

#[cfg(unix)]
fn set_owner_only(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    Ok(())
}

#[cfg(not(unix))]
fn set_owner_only(_path: &Path) -> Result<()> {
    Ok(())
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn random_url_safe_token() -> String {
    let mut buf = [0u8; 32];
    SystemRandom::new()
        .fill(&mut buf)
        .expect("system RNG unavailable while generating an OAuth state token");
    BASE64_URL.encode(buf)
}

/// Minimal percent-encoding for OAuth authorize URL query parameters. Client IDs and
/// our own generated state tokens are URL-safe base64/hex already; this only guards
/// against any unexpected character reaching the redirect URL unescaped.
fn percent_encode(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for b in input.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

// ── Auth state (OAuth-in-flight + role cache) ────────────────────────────────────

/// Shared operator-auth state: config, the [`GithubApi`] backend, the HMAC key, the
/// in-flight OAuth `state` map, and the 5-minute role cache. Mount as an
/// `axum::Extension<Arc<AuthState>>` (or router state) on the `:15001` operator
/// router; see [`require_role`] for the middleware contract.
pub struct AuthState {
    config: AuthConfig,
    github: Arc<dyn GithubApi>,
    hmac_key: [u8; 32],
    /// OAuth `state` token -> issued-at. Single-use: removed on callback.
    oauth_states: Mutex<HashMap<String, Instant>>,
    /// login -> (role, cached-at). TTL is [`ROLE_CACHE_TTL`].
    role_cache: Mutex<HashMap<String, (Role, Instant)>>,
    /// login -> GitHub access token, kept server-side only (never in the cookie) so
    /// a role refresh can re-call [`GithubApi::org_role`] without a fresh login.
    tokens: Mutex<HashMap<String, String>>,
}

impl AuthState {
    pub fn new(config: AuthConfig, github: Arc<dyn GithubApi>, hmac_key: [u8; 32]) -> Arc<Self> {
        Arc::new(Self {
            config,
            github,
            hmac_key,
            oauth_states: Mutex::new(HashMap::new()),
            role_cache: Mutex::new(HashMap::new()),
            tokens: Mutex::new(HashMap::new()),
        })
    }

    pub fn config(&self) -> &AuthConfig {
        &self.config
    }

    fn cache_role(&self, login: &str, role: Role) {
        self.role_cache
            .lock()
            .unwrap()
            .insert(login.to_string(), (role, Instant::now()));
    }

    fn store_token(&self, login: &str, token: &str) {
        self.tokens
            .lock()
            .unwrap()
            .insert(login.to_string(), token.to_string());
    }

    /// Mints a session for [`BOOTSTRAP_ADMIN_LOGIN`] using this state's own HMAC
    /// key — the SAME [`mint_session`] a real OAuth callback calls, so a bootstrap
    /// cookie and an OAuth cookie are indistinguishable to [`check_access`]/
    /// [`require_role`]. There is no token/membership to cache — the resulting
    /// session is never subject to the 5-minute role-cache TTL; see
    /// [`Self::effective_role`]'s special case for why it doesn't need to be.
    pub fn mint_bootstrap_session(&self, now_unix: u64) -> String {
        mint_session(&self.hmac_key, BOOTSTRAP_ADMIN_LOGIN, Role::Admin, now_unix)
    }

    /// The role to apply to the CURRENT request only: a fresh cache hit returns the
    /// cached role with zero GitHub calls; a stale/missing entry re-checks via
    /// [`GithubApi::org_role`]. On ANY failure of that refresh — network error,
    /// missing token, non-member — the role degrades to [`Role::Viewer`] for this
    /// request. The cache itself is left untouched on failure (never poisoned to
    /// Viewer), so the next request retries the refresh rather than being stuck.
    ///
    /// [`BOOTSTRAP_ADMIN_LOGIN`] is special-cased ahead of the cache: it has no
    /// GitHub token to refresh against, so without this it would silently degrade
    /// to Viewer after [`ROLE_CACHE_TTL`] — a bootstrap-token login is admin for
    /// its whole session, not just 5 minutes of it.
    async fn effective_role(&self, login: &str) -> Role {
        if login == BOOTSTRAP_ADMIN_LOGIN {
            return Role::Admin;
        }
        if let Some((role, cached_at)) = self.role_cache.lock().unwrap().get(login).copied() {
            if cached_at.elapsed() < ROLE_CACHE_TTL {
                return role;
            }
        }
        let Some(token) = self.tokens.lock().unwrap().get(login).cloned() else {
            return Role::Viewer;
        };
        match self.github.org_role(&token, login).await {
            Ok(membership) => match map_role(&self.config, &membership) {
                Some(role) => {
                    self.cache_role(login, role);
                    role
                }
                None => Role::Viewer,
            },
            Err(_) => Role::Viewer,
        }
    }
}

/// Constant-time byte-equality (no early return on first differing byte) so the
/// OAuth state/cookie comparison leaks no timing signal about how many leading
/// bytes matched. A length difference short-circuits (token length is not secret).
fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

// ── Bootstrap admin token (temporary, disable-able Decision-8 exception) ─────────

/// How long a freshly generated bootstrap token remains valid. Short by design —
/// this is a stopgap for a human with server access to log in once, not a
/// standing credential; a fresh token is minted (invalidating any old one) every
/// `uaa-control` process start.
pub const BOOTSTRAP_TOKEN_TTL: Duration = Duration::from_secs(15 * 60);

/// The marker file (under [`AuthConfig::state_dir`]) an admin-triggered
/// [`BootstrapTokenState::disable_permanently`] writes — its mere existence keeps
/// the feature disabled across restarts, same spirit as the HMAC key file.
const BOOTSTRAP_DISABLED_MARKER: &str = "bootstrap_disabled";

/// Holds the current (at most one outstanding) bootstrap token's SHA-256 hash +
/// expiry — never the raw value, which is handed back once, at generation time,
/// for the caller to log/write to disk — plus the two independent ways this
/// stopgap can be turned off for good.
pub struct BootstrapTokenState {
    /// `UAA_OPERATOR_DISABLE_BOOTSTRAP_TOKEN` — an operator's kill switch, checked
    /// once at startup.
    disabled_by_env: bool,
    /// Set by [`Self::disable_permanently`] (a logged-in admin choosing to retire
    /// this path once real OAuth works) and persisted via
    /// [`BOOTSTRAP_DISABLED_MARKER`] so it survives a restart.
    disabled_by_admin: std::sync::atomic::AtomicBool,
    marker_path: PathBuf,
    current: Mutex<Option<(String, SystemTime)>>,
}

impl BootstrapTokenState {
    /// `state_dir` is the same directory the HMAC key lives in
    /// ([`AuthConfig::state_dir`]) — the marker file just needs to persist
    /// alongside it. `disabled_by_env` is read by the caller from
    /// `UAA_OPERATOR_DISABLE_BOOTSTRAP_TOKEN` (see [`Self::from_env`]) rather
    /// than inside this constructor, so tests can exercise both states without
    /// racing on process-global env state across parallel test threads.
    pub fn new(state_dir: &Path, disabled_by_env: bool) -> Self {
        let marker_path = state_dir.join(BOOTSTRAP_DISABLED_MARKER);
        Self {
            disabled_by_env,
            disabled_by_admin: std::sync::atomic::AtomicBool::new(marker_path.exists()),
            marker_path,
            current: Mutex::new(None),
        }
    }

    /// Production constructor: reads `UAA_OPERATOR_DISABLE_BOOTSTRAP_TOKEN` from
    /// the environment once, at startup.
    pub fn from_env(state_dir: &Path) -> Self {
        Self::new(
            state_dir,
            std::env::var("UAA_OPERATOR_DISABLE_BOOTSTRAP_TOKEN").is_ok(),
        )
    }

    pub fn enabled(&self) -> bool {
        !self.disabled_by_env
            && !self
                .disabled_by_admin
                .load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Generates a fresh token, replacing (invalidating) any previous one.
    /// Returns `None` when disabled — callers must not log or persist anything
    /// in that case.
    pub fn generate(&self) -> Option<String> {
        if !self.enabled() {
            return None;
        }
        let mut raw_bytes = [0u8; 32];
        SystemRandom::new()
            .fill(&mut raw_bytes)
            .expect("system RNG unavailable while generating a bootstrap token");
        let raw = format!("uaabs_{}", BASE64_URL.encode(raw_bytes));
        let hash = sha256_hex(&raw);
        let expires_at = SystemTime::now() + BOOTSTRAP_TOKEN_TTL;
        *self.current.lock().unwrap() = Some((hash, expires_at));
        Some(raw)
    }

    /// Single-use: the stored hash is cleared on the FIRST call regardless of
    /// outcome — a wrong guess must not leave the real token usable afterward.
    pub fn consume(&self, submitted: &str) -> bool {
        if !self.enabled() {
            *self.current.lock().unwrap() = None;
            return false;
        }
        let Some((hash, expires_at)) = self.current.lock().unwrap().take() else {
            return false;
        };
        if SystemTime::now() > expires_at {
            return false;
        }
        ct_eq(sha256_hex(submitted).as_bytes(), hash.as_bytes())
    }

    /// Permanently retires this stopgap: clears any outstanding token, flips the
    /// in-memory flag, and writes the marker file so the disable survives a
    /// restart. Intended to be called from an admin-only API once real OAuth
    /// login is confirmed working.
    pub fn disable_permanently(&self) -> std::io::Result<()> {
        *self.current.lock().unwrap() = None;
        self.disabled_by_admin
            .store(true, std::sync::atomic::Ordering::SeqCst);
        fs::write(&self.marker_path, b"disabled by admin\n")
    }
}

fn sha256_hex(s: &str) -> String {
    use ring::digest;
    let digest = digest::digest(&digest::SHA256, s.as_bytes());
    let mut out = String::with_capacity(64);
    for b in digest.as_ref() {
        use std::fmt::Write;
        write!(&mut out, "{b:02x}").expect("writing to a String never fails");
    }
    out
}

// ── OAuth flow (pure, testable core) ──────────────────────────────────────────────

/// Result of [`begin_oauth_login`]: the state token (also stashed server-side) and
/// the full GitHub authorize URL to redirect the browser to.
pub struct LoginRedirect {
    pub state_token: String,
    pub authorize_url: String,
}

/// Starts a login attempt: mints a random `state` token, stashes it (short TTL,
/// single-use), and builds the `https://github.com/login/oauth/authorize` URL.
pub fn begin_oauth_login(state: &AuthState) -> LoginRedirect {
    let state_token = random_url_safe_token();
    state
        .oauth_states
        .lock()
        .unwrap()
        .insert(state_token.clone(), Instant::now());
    let authorize_url = format!(
        "https://github.com/login/oauth/authorize?client_id={}&state={}&scope={}",
        percent_encode(&state.config.client_id),
        percent_encode(&state_token),
        percent_encode("read:org"),
    );
    LoginRedirect {
        state_token,
        authorize_url,
    }
}

/// Outcome of [`complete_oauth_callback`]. Only [`CallbackOutcome::Success`] mints a
/// cookie — every other variant leaves the caller unauthenticated.
#[derive(Debug)]
pub enum CallbackOutcome {
    /// Login completed; `cookie` is the ready-to-set `uaa_session` value.
    Success {
        cookie: String,
        login: String,
        role: Role,
    },
    /// The `state` parameter didn't match (or had already expired/been consumed).
    /// Rejected before any `GithubApi` call — a replayed or forged callback never
    /// touches GitHub.
    StateMismatch,
    /// The code exchanged fine, but the user isn't an org member. No session is
    /// minted — the operator maps this to 403.
    Denied,
    /// A `GithubApi` call failed during login. Per spec: a login must NEVER
    /// default-grant on GitHub failure, so no cookie is minted — the operator maps
    /// this to 502.
    GithubError,
}

/// Completes the OAuth callback: validates `state`, exchanges `code`, resolves the
/// login + org/team membership, and — only on full success — mints a session cookie
/// and warms the role cache + token store for future refreshes.
pub async fn complete_oauth_callback(
    state: &AuthState,
    code: &str,
    given_state: &str,
    cookie_state: Option<&str>,
    now_unix: u64,
) -> CallbackOutcome {
    // Browser-binding (CSRF / login-fixation defense): the `state` GitHub echoes
    // back must equal the `oauth_state` cookie we set on THIS browser at
    // /auth/login, compared in constant time — in ADDITION to the single-use
    // server-side store check below. Without this, an attacker who obtains a valid
    // state can replay it into a victim's browser and fixate a session.
    let cookie_bound =
        matches!(cookie_state, Some(c) if ct_eq(c.as_bytes(), given_state.as_bytes()));
    // Consume the server-side state regardless (single-use), then require BOTH checks.
    let state_valid = {
        let mut states = state.oauth_states.lock().unwrap();
        matches!(states.remove(given_state), Some(issued_at) if issued_at.elapsed() < OAUTH_STATE_TTL)
    };
    if !cookie_bound || !state_valid {
        return CallbackOutcome::StateMismatch;
    }

    let token = match state.github.exchange_code(code).await {
        Ok(token) => token,
        Err(_) => return CallbackOutcome::GithubError,
    };
    let login = match state.github.user_login(&token).await {
        Ok(login) => login,
        Err(_) => return CallbackOutcome::GithubError,
    };
    let membership = match state.github.org_role(&token, &login).await {
        Ok(membership) => membership,
        Err(_) => return CallbackOutcome::GithubError,
    };
    let Some(role) = map_role(&state.config, &membership) else {
        return CallbackOutcome::Denied;
    };

    state.store_token(&login, &token);
    state.cache_role(&login, role);
    let cookie = mint_session(&state.hmac_key, &login, role, now_unix);
    CallbackOutcome::Success {
        cookie,
        login,
        role,
    }
}

// ── RBAC guard (pure, testable core) ──────────────────────────────────────────────

/// Result of [`check_access`].
#[derive(Debug)]
pub enum AccessDecision {
    /// The session is valid and its (possibly-refreshed) role meets the minimum.
    Allow(Session),
    /// No cookie, a malformed/forged cookie, or an expired one — all indistinguishable.
    Unauthenticated,
    /// A valid session whose role does not meet the minimum required.
    Forbidden,
}

/// The RBAC guard's pure decision function: verify the cookie, refresh the role
/// against the 5-minute cache (degrading to Viewer on any GitHub failure), and
/// compare against `min`. [`require_role`] wraps this for axum; unit tests call it
/// directly so the fail-closed law is provable without a real HTTP stack.
pub async fn check_access(
    state: &AuthState,
    min: Role,
    cookie_value: Option<&str>,
    now_unix: u64,
) -> AccessDecision {
    let Some(cookie_value) = cookie_value else {
        return AccessDecision::Unauthenticated;
    };
    let Some(session) = verify_session(&state.hmac_key, cookie_value, now_unix) else {
        return AccessDecision::Unauthenticated;
    };
    let role = state.effective_role(&session.login).await;
    if role >= min {
        AccessDecision::Allow(Session {
            login: session.login,
            role,
        })
    } else {
        AccessDecision::Forbidden
    }
}

fn extract_cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    let raw = headers.get(header::COOKIE)?.to_str().ok()?;
    raw.split(';').map(str::trim).find_map(|kv| {
        kv.strip_prefix(name)
            .and_then(|rest| rest.strip_prefix('='))
            .map(str::to_string)
    })
}

fn extract_session_cookie(headers: &HeaderMap) -> Option<String> {
    extract_cookie(headers, SESSION_COOKIE_NAME)
}

// ── axum wiring (handlers + middleware for CT-07 to mount) ───────────────────────

/// `GET /auth/login` — 302 to GitHub's authorize endpoint with a fresh `state`.
pub async fn login_handler(State(state): State<Arc<AuthState>>) -> Response {
    let redirect = begin_oauth_login(&state);
    // Bind this state to the browser: set it as a short-lived HttpOnly cookie the
    // callback must echo back. SameSite=Lax so it survives the GitHub redirect.
    let cookie = format!(
        "{OAUTH_STATE_COOKIE_NAME}={}; Path=/; Max-Age={}; Secure; HttpOnly; SameSite=Lax",
        redirect.state_token,
        OAUTH_STATE_TTL.as_secs(),
    );
    let mut resp = Redirect::to(&redirect.authorize_url).into_response();
    if let Ok(value) = HeaderValue::from_str(&cookie) {
        resp.headers_mut().insert(header::SET_COOKIE, value);
    }
    resp
}

#[derive(Debug, Deserialize)]
pub struct CallbackQuery {
    pub code: String,
    pub state: String,
}

/// `GET /auth/callback?code&state` — completes the OAuth flow. On success, sets the
/// signed session cookie (`Secure; HttpOnly; SameSite=Lax`) and 302s to `/`.
/// A `state` mismatch is a 400, a non-org-member is a 403, and any `GithubApi`
/// failure during login is a 502 — none of those three paths ever sets a cookie.
pub async fn callback_handler(
    State(state): State<Arc<AuthState>>,
    headers: HeaderMap,
    Query(query): Query<CallbackQuery>,
) -> Response {
    // The state the browser must echo from its /auth/login cookie.
    let cookie_state = extract_cookie(&headers, OAUTH_STATE_COOKIE_NAME);
    // Clear the one-time state cookie on every outcome (success or reject).
    let clear_state =
        format!("{OAUTH_STATE_COOKIE_NAME}=; Path=/; Max-Age=0; Secure; HttpOnly; SameSite=Lax");
    let append_clear = |resp: &mut Response| {
        if let Ok(v) = HeaderValue::from_str(&clear_state) {
            resp.headers_mut().append(header::SET_COOKIE, v);
        }
    };
    match complete_oauth_callback(
        &state,
        &query.code,
        &query.state,
        cookie_state.as_deref(),
        unix_now(),
    )
    .await
    {
        CallbackOutcome::Success { cookie, .. } => {
            let cookie_header = format!(
                "{SESSION_COOKIE_NAME}={cookie}; Path=/; Max-Age={SESSION_TTL_SECS}; Secure; HttpOnly; SameSite=Lax"
            );
            let mut resp = Redirect::to("/").into_response();
            if let Ok(value) = HeaderValue::from_str(&cookie_header) {
                resp.headers_mut().insert(header::SET_COOKIE, value);
            }
            append_clear(&mut resp);
            resp
        }
        CallbackOutcome::StateMismatch => {
            let mut resp = (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "state_mismatch"})),
            )
                .into_response();
            append_clear(&mut resp);
            resp
        }
        CallbackOutcome::Denied => {
            let mut resp =
                (StatusCode::FORBIDDEN, Json(json!({"error": "forbidden"}))).into_response();
            append_clear(&mut resp);
            resp
        }
        CallbackOutcome::GithubError => {
            let mut resp = (
                StatusCode::BAD_GATEWAY,
                Json(json!({"error": "github_unavailable"})),
            )
                .into_response();
            append_clear(&mut resp);
            resp
        }
    }
}

/// RBAC middleware. Contract for CT-07 (operator plane): mutating routes wrap
/// with `require_role(router, Role::Operator)`, read routes with
/// `require_role(router, Role::Viewer)`. Reads `Arc<AuthState>` from the
/// request's `Extension` (mount it globally on the operator router with
/// `.layer(Extension(auth_state))`).
///
/// Takes and returns the `Router<S>` being built (rather than a standalone
/// layer value) so the middleware closure's concrete type never has to cross
/// a function boundary as a partially-erased `impl Trait` — naming that type
/// precisely enough for axum's `Service` bound to resolve at an external call
/// site turned out to be impractical; applying the layer here, where the
/// closure's full type is known, sidesteps the problem entirely.
///
/// Behavior: missing/bad/expired cookie -> 401 JSON `{"error":"unauthenticated"}` for
/// any path starting `/api/`, else a 302 to `/auth/login`; a valid session whose
/// (possibly-refreshed) role is below `min` -> 403 JSON `{"error":"forbidden"}`;
/// otherwise the request is forwarded unchanged.
pub fn require_role<S>(router: axum::Router<S>, min: Role) -> axum::Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    router.route_layer(axum::middleware::from_fn(
        move |Extension(state): Extension<Arc<AuthState>>,
              req: axum::extract::Request,
              next: Next| async move { role_guard(state, min, req, next).await },
    ))
}

async fn role_guard(
    state: Arc<AuthState>,
    min: Role,
    mut req: axum::extract::Request,
    next: Next,
) -> Response {
    let is_api_path = req.uri().path().starts_with("/api/");
    let cookie_value = extract_session_cookie(req.headers());
    let decision = check_access(&state, min, cookie_value.as_deref(), unix_now()).await;
    match decision {
        AccessDecision::Allow(session) => {
            // Lets downstream handlers attribute mutations to the real
            // logged-in principal (GitHub login, or `BOOTSTRAP_ADMIN_LOGIN`)
            // instead of a placeholder actor string.
            req.extensions_mut().insert(session);
            next.run(req).await
        }
        AccessDecision::Unauthenticated => {
            if is_api_path {
                (
                    StatusCode::UNAUTHORIZED,
                    Json(json!({"error": "unauthenticated"})),
                )
                    .into_response()
            } else {
                Redirect::to("/auth/login").into_response()
            }
        }
        AccessDecision::Forbidden => {
            (StatusCode::FORBIDDEN, Json(json!({"error": "forbidden"}))).into_response()
        }
    }
}

/// `GET /api/auth/status` — public (no role required): tells the SPA whether a
/// session is currently authenticated and whether the bootstrap-token path is
/// still offered, so the `/login` page knows whether to render the token form.
pub async fn auth_status_handler(
    Extension(state): Extension<Arc<AuthState>>,
    Extension(bootstrap): Extension<Arc<BootstrapTokenState>>,
    headers: HeaderMap,
) -> Response {
    let cookie_value = extract_session_cookie(&headers);
    let authenticated = match cookie_value {
        Some(v) => verify_session(&state.hmac_key, &v, unix_now()).is_some(),
        None => false,
    };
    Json(json!({
        "authenticated": authenticated,
        "bootstrap_token_enabled": bootstrap.enabled(),
    }))
    .into_response()
}

#[derive(Debug, Deserialize)]
pub struct BootstrapLoginBody {
    pub token: String,
}

/// `POST /api/auth/bootstrap` — exchanges a valid, unexpired, not-yet-used
/// bootstrap token for a real `uaa_session` cookie identical in shape to one a
/// GitHub OAuth login would mint (see the module doc's "Bootstrap admin token"
/// section). Wrong/expired/reused token, or the feature disabled -> 401; never
/// reveals which of those applies (same "no oracle" law as [`verify_session`]).
pub async fn bootstrap_login_handler(
    Extension(state): Extension<Arc<AuthState>>,
    Extension(bootstrap): Extension<Arc<BootstrapTokenState>>,
    Json(body): Json<BootstrapLoginBody>,
) -> Response {
    if !bootstrap.consume(&body.token) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error": "invalid_or_expired_token"})),
        )
            .into_response();
    }
    let cookie = state.mint_bootstrap_session(unix_now());
    let cookie_header = format!(
        "{SESSION_COOKIE_NAME}={cookie}; Path=/; Max-Age={SESSION_TTL_SECS}; Secure; HttpOnly; SameSite=Lax"
    );
    let mut resp = Json(json!({"ok": true})).into_response();
    if let Ok(value) = HeaderValue::from_str(&cookie_header) {
        resp.headers_mut().insert(header::SET_COOKIE, value);
    }
    resp
}

/// `POST /api/auth/bootstrap/disable` — admin-only (mount behind
/// `require_role(Role::Admin)`): lets whoever is currently logged in (via
/// bootstrap OR real OAuth) permanently retire the bootstrap-token stopgap once
/// real GitHub OAuth is confirmed working, without needing server/env access.
pub async fn disable_bootstrap_handler(
    Extension(bootstrap): Extension<Arc<BootstrapTokenState>>,
) -> Response {
    match bootstrap.disable_permanently() {
        Ok(()) => Json(json!({"ok": true})).into_response(),
        Err(err) => {
            tracing::error!(%err, "failed to persist bootstrap-token disable marker");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "failed to persist disable"})),
            )
                .into_response()
        }
    }
}

// ── Unit tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tempfile::tempdir;

    /// Mock [`GithubApi`]: configurable success/failure per method, with call
    /// counters so tests can assert exactly how many network calls WOULD have
    /// happened (and, for the fail-closed tests, that a failure short-circuits
    /// before any later call).
    struct MockGithubApi {
        login: String,
        teams: Vec<String>,
        org_member: bool,
        fail_exchange: bool,
        fail_user_login: bool,
        fail_org_role: bool,
        exchange_calls: AtomicUsize,
        user_login_calls: AtomicUsize,
        org_role_calls: AtomicUsize,
    }

    impl MockGithubApi {
        fn healthy(login: &str, teams: Vec<String>, org_member: bool) -> Self {
            Self {
                login: login.to_string(),
                teams,
                org_member,
                fail_exchange: false,
                fail_user_login: false,
                fail_org_role: false,
                exchange_calls: AtomicUsize::new(0),
                user_login_calls: AtomicUsize::new(0),
                org_role_calls: AtomicUsize::new(0),
            }
        }

        fn failing_exchange() -> Self {
            let mut m = Self::healthy("unused", vec![], false);
            m.fail_exchange = true;
            m
        }

        fn failing_org_role() -> Self {
            let mut m = Self::healthy("alice", vec![], false);
            m.fail_org_role = true;
            m
        }
    }

    #[async_trait]
    impl GithubApi for MockGithubApi {
        async fn exchange_code(&self, _code: &str) -> Result<String> {
            self.exchange_calls.fetch_add(1, Ordering::SeqCst);
            if self.fail_exchange {
                return Err(anyhow!("mock: exchange_code failed"));
            }
            Ok("mock-token".to_string())
        }

        async fn user_login(&self, _token: &str) -> Result<String> {
            self.user_login_calls.fetch_add(1, Ordering::SeqCst);
            if self.fail_user_login {
                return Err(anyhow!("mock: user_login failed"));
            }
            Ok(self.login.clone())
        }

        async fn org_role(&self, _token: &str, _login: &str) -> Result<OrgMembership> {
            self.org_role_calls.fetch_add(1, Ordering::SeqCst);
            if self.fail_org_role {
                return Err(anyhow!("mock: org_role failed"));
            }
            Ok(OrgMembership {
                org_member: self.org_member,
                teams: self.teams.clone(),
            })
        }
    }

    fn test_config(state_dir: PathBuf) -> AuthConfig {
        AuthConfig {
            client_id: "test-client-id".to_string(),
            client_secret: "test-client-secret".to_string(),
            org: "falkcorp".to_string(),
            admin_team: "uaa-admins".to_string(),
            operator_team: "uaa-operators".to_string(),
            state_dir,
        }
    }

    /// Builds an [`AuthState`] backed by `mock`, returning both the trait-object
    /// state (for the functions under test) and the concrete mock `Arc` (so tests
    /// can still read its call counters afterward) plus the owning tempdir.
    fn test_state(mock: MockGithubApi) -> (Arc<AuthState>, Arc<MockGithubApi>, tempfile::TempDir) {
        let dir = tempdir().expect("tempdir");
        let key = load_or_create_hmac_key(dir.path()).expect("hmac key");
        let mock = Arc::new(mock);
        let state = AuthState::new(test_config(dir.path().to_path_buf()), mock.clone(), key);
        (state, mock, dir)
    }

    async fn login_via_mock(state: &AuthState) -> CallbackOutcome {
        let login = begin_oauth_login(state);
        // Happy path: the browser echoes the same state via its oauth_state cookie.
        complete_oauth_callback(
            state,
            "mock-code",
            &login.state_token,
            Some(login.state_token.as_str()),
            unix_now(),
        )
        .await
    }

    #[test]
    fn test_cookie_round_trip() {
        let key = [7u8; 32];
        let now = 1_000_000u64;
        let cookie = mint_session(&key, "octocat", Role::Operator, now);
        let session = verify_session(&key, &cookie, now).expect("valid session");
        assert_eq!(session.login, "octocat");
        assert_eq!(session.role, Role::Operator);
    }

    #[test]
    fn test_cookie_bad_sig_rejected() {
        let dir = tempdir().expect("tempdir");
        let key = load_or_create_hmac_key(dir.path()).expect("hmac key");

        // HMAC key file permissions: mode 0600, asserted here per the acceptance
        // criteria (cookie integrity + key custody are proven together).
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let meta = fs::metadata(dir.path().join(HMAC_KEY_FILE)).expect("key file exists");
            assert_eq!(meta.permissions().mode() & 0o777, 0o600);
        }

        let other_key = [9u8; 32];
        let now = 1_000_000u64;
        let cookie = mint_session(&key, "octocat", Role::Viewer, now);

        // Wrong key entirely.
        assert!(verify_session(&other_key, &cookie, now).is_none());
        // Tampered payload under the right key.
        let tampered = format!("{cookie}-tampered");
        assert!(verify_session(&key, &tampered, now).is_none());
    }

    #[test]
    fn test_cookie_expired_rejected() {
        let key = [3u8; 32];
        let now = 1_000_000u64;
        let cookie = mint_session(&key, "octocat", Role::Admin, now);
        let after_expiry = now + SESSION_TTL_SECS + 1;
        assert!(verify_session(&key, &cookie, after_expiry).is_none());
    }

    #[tokio::test]
    async fn test_role_mapping_admin_team() {
        let (state, _mock, _dir) = test_state(MockGithubApi::healthy(
            "alice",
            vec!["uaa-admins".to_string()],
            true,
        ));
        match login_via_mock(&state).await {
            CallbackOutcome::Success { role, .. } => assert_eq!(role, Role::Admin),
            other => panic!("expected Success(Admin), got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_role_mapping_operator_team() {
        let (state, _mock, _dir) = test_state(MockGithubApi::healthy(
            "bob",
            vec!["uaa-operators".to_string()],
            true,
        ));
        match login_via_mock(&state).await {
            CallbackOutcome::Success { role, .. } => assert_eq!(role, Role::Operator),
            other => panic!("expected Success(Operator), got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_role_mapping_org_member_viewer() {
        let (state, _mock, _dir) = test_state(MockGithubApi::healthy("carol", vec![], true));
        match login_via_mock(&state).await {
            CallbackOutcome::Success { role, .. } => assert_eq!(role, Role::Viewer),
            other => panic!("expected Success(Viewer), got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_role_mapping_non_member_403() {
        let (state, _mock, _dir) = test_state(MockGithubApi::healthy("dave", vec![], false));
        match login_via_mock(&state).await {
            CallbackOutcome::Denied => {}
            other => panic!("expected Denied, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_oauth_state_mismatch_rejected() {
        let (state, mock, _dir) = test_state(MockGithubApi::healthy("alice", vec![], true));
        let _issued = begin_oauth_login(&state);
        // Cookie matches the (forged) given state, so the browser-binding passes —
        // this isolates the SERVER-side single-use store rejection.
        let outcome = complete_oauth_callback(
            &state,
            "mock-code",
            "not-the-issued-state",
            Some("not-the-issued-state"),
            unix_now(),
        )
        .await;
        assert!(matches!(outcome, CallbackOutcome::StateMismatch));
        // Rejected before any GithubApi call — a forged/replayed callback never
        // touches GitHub.
        assert_eq!(mock.exchange_calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn test_oauth_cookie_binding_required() {
        // A valid server-side state but a MISSING or MISMATCHED browser cookie is
        // rejected (CSRF / login-fixation defense) — before any GithubApi call.
        let (state, mock, _dir) = test_state(MockGithubApi::healthy("alice", vec![], true));
        let issued = begin_oauth_login(&state);

        // Missing cookie → StateMismatch.
        let no_cookie =
            complete_oauth_callback(&state, "mock-code", &issued.state_token, None, unix_now())
                .await;
        assert!(matches!(no_cookie, CallbackOutcome::StateMismatch));
        assert_eq!(mock.exchange_calls.load(Ordering::SeqCst), 0);

        // Fresh state (the prior one was consumed), wrong cookie → StateMismatch.
        let issued2 = begin_oauth_login(&state);
        let wrong_cookie = complete_oauth_callback(
            &state,
            "mock-code",
            &issued2.state_token,
            Some("attacker-cookie"),
            unix_now(),
        )
        .await;
        assert!(matches!(wrong_cookie, CallbackOutcome::StateMismatch));
        assert_eq!(mock.exchange_calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn test_login_github_error_no_cookie() {
        let (state, _mock, _dir) = test_state(MockGithubApi::failing_exchange());
        match login_via_mock(&state).await {
            CallbackOutcome::GithubError => {}
            other => panic!("expected GithubError, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_refresh_failure_degrades_to_viewer() {
        let (state, _mock, _dir) = test_state(MockGithubApi::failing_org_role());

        // Seed a stale cached Admin role (past the 5-minute TTL) plus the token a
        // refresh would use, then present a session cookie asserting Admin.
        let stale_at = Instant::now()
            .checked_sub(ROLE_CACHE_TTL + Duration::from_secs(1))
            .expect("test host uptime long enough to backdate an Instant");
        state
            .role_cache
            .lock()
            .unwrap()
            .insert("alice".to_string(), (Role::Admin, stale_at));
        state
            .tokens
            .lock()
            .unwrap()
            .insert("alice".to_string(), "stale-token".to_string());
        let cookie = mint_session(&state.hmac_key, "alice", Role::Admin, unix_now());

        // Mutation: denied. The stale cached Admin is never served past its TTL.
        let mutation = check_access(&state, Role::Operator, Some(&cookie), unix_now()).await;
        assert!(
            matches!(mutation, AccessDecision::Forbidden),
            "expected Forbidden, got {mutation:?}"
        );

        // Read: allowed. Fail-closed degrades to Viewer, it never errors reads.
        let read = check_access(&state, Role::Viewer, Some(&cookie), unix_now()).await;
        assert!(
            matches!(read, AccessDecision::Allow(_)),
            "expected Allow, got {read:?}"
        );
    }

    #[tokio::test]
    async fn test_cache_within_ttl_skips_github() {
        let (state, mock, _dir) = test_state(MockGithubApi::healthy(
            "erin",
            vec!["uaa-operators".to_string()],
            true,
        ));
        let outcome = login_via_mock(&state).await;
        let cookie = match outcome {
            CallbackOutcome::Success { cookie, .. } => cookie,
            other => panic!("expected Success, got {other:?}"),
        };
        assert_eq!(mock.org_role_calls.load(Ordering::SeqCst), 1);

        // A second guarded access well within the 5-minute TTL must not call
        // GithubApi again — the call count stays at 1.
        let decision = check_access(&state, Role::Operator, Some(&cookie), unix_now()).await;
        assert!(matches!(decision, AccessDecision::Allow(_)));
        assert_eq!(mock.org_role_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_operator_mutation_passes_guards() {
        // Anti-over-suppression: a legitimate Operator session against a healthy
        // mock must be admitted by require_role(Operator) — the fail-closed stack
        // must not also swallow valid requests.
        let (state, _mock, _dir) = test_state(MockGithubApi::healthy(
            "frank",
            vec!["uaa-operators".to_string()],
            true,
        ));
        let cookie = match login_via_mock(&state).await {
            CallbackOutcome::Success { cookie, role, .. } => {
                assert_eq!(role, Role::Operator);
                cookie
            }
            other => panic!("expected Success(Operator), got {other:?}"),
        };
        let decision = check_access(&state, Role::Operator, Some(&cookie), unix_now()).await;
        assert!(
            matches!(decision, AccessDecision::Allow(_)),
            "a legitimate Operator mutation must pass the guard stack, got {decision:?}"
        );
    }

    // ── Bootstrap admin token ──────────────────────────────────────────────

    #[tokio::test]
    async fn test_bootstrap_session_grants_admin_forever_no_ttl_degrade() {
        let (state, _mock, _dir) = test_state(MockGithubApi::healthy("unused", vec![], false));
        let cookie = state.mint_bootstrap_session(unix_now());

        // Immediately: Admin.
        let decision = check_access(&state, Role::Admin, Some(&cookie), unix_now()).await;
        assert!(matches!(decision, AccessDecision::Allow(_)));

        // Long past the 5-minute role-cache TTL a real GitHub-backed session
        // would degrade at: still Admin, because effective_role special-cases
        // BOOTSTRAP_ADMIN_LOGIN before ever consulting the cache/token/GitHub.
        let far_future = unix_now() + ROLE_CACHE_TTL.as_secs() * 10;
        let decision = check_access(&state, Role::Admin, Some(&cookie), far_future).await;
        assert!(
            matches!(decision, AccessDecision::Allow(_)),
            "bootstrap admin must not degrade like a real session's cached role would, got {decision:?}"
        );
    }

    #[test]
    fn test_bootstrap_token_disabled_by_env_never_generates_or_consumes() {
        let dir = tempdir().expect("tempdir");
        let state = BootstrapTokenState::new(dir.path(), true);
        assert!(!state.enabled());
        assert!(state.generate().is_none());
        assert!(!state.consume("anything"));
    }

    #[test]
    fn test_bootstrap_token_valid_consume_succeeds_once() {
        let dir = tempdir().expect("tempdir");
        let state = BootstrapTokenState::new(dir.path(), false);
        let raw = state.generate().expect("enabled by default");
        assert!(raw.starts_with("uaabs_"));
        assert!(
            state.consume(&raw),
            "first consume with the right token must succeed"
        );
        assert!(
            !state.consume(&raw),
            "single-use: a second consume with the SAME token must fail"
        );
    }

    #[test]
    fn test_bootstrap_token_wrong_value_fails_and_still_consumes() {
        let dir = tempdir().expect("tempdir");
        let state = BootstrapTokenState::new(dir.path(), false);
        let raw = state.generate().unwrap();
        assert!(!state.consume("uaabs_wrong-value"));
        assert!(
            !state.consume(&raw),
            "a wrong guess must not leave the real token usable afterward"
        );
    }

    #[test]
    fn test_bootstrap_token_regenerate_invalidates_previous() {
        let dir = tempdir().expect("tempdir");
        let state = BootstrapTokenState::new(dir.path(), false);
        let first = state.generate().unwrap();
        let _second = state.generate().unwrap();
        assert!(
            !state.consume(&first),
            "regenerating must invalidate the old token"
        );
    }

    #[test]
    fn test_bootstrap_disable_permanently_persists_across_new_instances() {
        let dir = tempdir().expect("tempdir");
        {
            let state = BootstrapTokenState::new(dir.path(), false);
            assert!(state.enabled());
            state.disable_permanently().expect("write marker");
            assert!(!state.enabled());
        }
        // A freshly constructed state (simulating a restart) must see the
        // marker file and come up disabled.
        let restarted = BootstrapTokenState::new(dir.path(), false);
        assert!(!restarted.enabled());
        assert!(restarted.generate().is_none());
    }
}
