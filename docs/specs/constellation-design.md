<!-- file: docs/specs/constellation-design.md -->
<!-- version: 1.2.0 -->
<!-- guid: 49fbec09-b12d-402c-9f51-5aee6ff92d70 -->
<!-- last-edited: 2026-07-14 -->

# uaa Constellation — Design Spec

**Status:** Draft
**Scope:** Rust (new cargo workspace, new service binaries), protobuf, Node SPA (build tooling only), docs. NO server (172.16.2.30) writes, NO hardware actions — everything validated in-repo (cargo) and in the QEMU+swtpm harness.
**Judge panel:** 3-lens adversarial review (correctness / ops-rollback / simplicity-scope) run 2026-07-10; all CHALLENGED verdicts merged below — each decision carries its verdict and, where the owner's operation brief locks the choice, the surviving objection is recorded rather than acted on.

---

## Motivation

The running netboot/install plane is almost entirely un-version-controlled: a ~731-line
Python stdlib `http.server` on 172.16.2.30:25000 (`scripts/autoinstall-agent.py` is a
tracked mirror), plus server-only shell scripts (`register-*.sh`, `reporting.sh`),
nginx/dnsmasq/tftpd configs, and hand-placed seeds under `/var/www/html`. The service is
plain HTTP, single-threaded, unauthenticated, and trusts `ip neigh` MAC resolution —
explicitly spoofable on-subnet (see `docs/specs/install-server-design.md`, threat note).
The repo's Rust binary (`uaa`) is a mature installer/CLI with 311 passing lib tests and a
proven static-musl build, but zero serving capability (no axum/hyper/tonic — grep-verified
2026-07-10: `grep -rnE 'TcpListener|axum::|Router::' src/` → 0 hits).

Passive discovery of unknown PXE-booting machines does not exist. Machine identity is
un-attested. Operator actions (approve/flip) are unauthenticated GETs. Secrets placement
is safe (server-local `deploy-usb-configs.sh --inject-from`) but shell-fragile.

**Goal:** replace the Python/shell install plane with a small constellation of
self-contained static Rust services — versioned gRPC internally, JSON+OpenAPI for the
browser, PKI-enrolled agents, GitHub-OAuth operators — planned here, built by the
task-brief package, validated end-to-end in the VM harness before any hardware touch.

## Goals

- Drop-in parity with the Python :25000 machine plane, then retire it (parity-then-switch).
- Machine identity by enrollment PKI (CSR → operator approve → cert → mTLS), replacing
  `ip neigh` trust for everything except first-contact seed fetch.
- Operator plane: GitHub OAuth + org/team RBAC, JSON+OpenAPI, SPA served by the central
  Rust binary. Signed, hash-chained audit log for every mutating action.
- P0 features: discovery inbox (unknown PXE MACs → pending queue), one-click reinstall
  (flip + power + watch), config templating (reuse `render.rs`), post-install verify/drift
  sweep (reuse `verify.rs`), signed audit log.
- Every artifact a single static musl binary with signed self-update from the
  constellation (no public phone-home).
- Retire the ported shell tools (`make-ssh-ready-iso.sh`, `deploy-usb-configs.sh`,
  `build-installer-image.sh`, `vm-validate.sh`) only after their Rust replacement passes
  its gate.

## Non-goals (v1)

- No k8s, no config-management engine, no SSO/SAML, no HA DHCP/DNS replacement, no
  multi-tenancy (deliberate anti-MAAS/Foreman/Tinkerbell/Sidero posture).
- No WebAuthn/login-auth/break-glass/PIV-mTLS operator auth — GitHub OAuth ONLY.
- YubiKeys are NOT auth: the only YubiKey work is FIDO2+PIN LUKS keyslot management.
- No secret ever transits HTTP — `REPLACE_AT_PLACE_TIME` placeholders stay; injection is
  server-local only.
- No replacement of tftpd-hpa or the dnsmasq proxy-DHCP role itself (uaa-pxe *manages*
  their config; it does not reimplement DHCP/TFTP).
- nginx keeps serving the unrelated media site; it just leaves the uaa trust path.
- uaa degraded mode does NOT cover total CRDB quorum loss — that recovery is an
  out-of-band DB-layer operation (cockroach node recovery / backup restore), stated
  explicitly so nobody mistakes the snapshot/WAL for a quorum-loss bootstrap.

## Decisions (locked during design)

Verdicts: ✅ = judge-CONFIRMED, ⚡ = judge-CHALLENGED and repaired here, 🔒 =
judge-CHALLENGED but locked by the owner's operation brief (objection recorded).

1. 🔒 **Constellation of static musl binaries, one artifact per responsibility** —
   rejected: monolith (`uaa-server` with control/web/pxe as in-process modules — the
   simplicity judge's alternative, noting all three daemons share one host, one failure
   domain, and the SAGA fails closed if any peer is down; the owner locks the
   constellation for independent update/restart blast radius and a stable versioned
   API), and rejected: keeping Python+shell. **Repair adopted from the challenge:** the
   internal-daemon mTLS bootstrap is now specified — see Decision 23 (service certs are
   minted server-locally at install time, never via operator-approved enrollment).
2. ✅ **Internal transport = gRPC + protobuf via tonic/prost, compiled with pure-Rust
   `protox`** (no protoc C dependency, musl-clean) — rejected: JSON-RPC internal (no
   schema evolution), rejected: GraphQL internal (wrong tool for service-to-service).
   Proto style aligns with org house style (buf.build/falkcorp/gcommon).
3. ✅ **Browser API = JSON + OpenAPI (axum + utoipa)** — rejected (named, owner open to
   revisit): GraphQL via async-graphql; ~15 operations don't need a query language.
   Browsers never speak to workers directly; tonic-web is the escape hatch if that changes.
4. ⚡ **Registry system-of-record = CockroachDB** (fleet-native; survives the server
   itself being rebuilt, which flat JSON and SQLite both lose) — rejected: SQLite.
   Degraded mode (CRDB unreachable): reads served from a local snapshot
   (`/var/lib/uaa/registry-snapshot.json`, rewritten tmp+rename after every successful
   mutation), mutations 503 fail-closed EXCEPT telemetry ingestion
   (webhook/checkin/events), which appends to `/var/lib/uaa/wal.jsonl`. **Repairs:**
   (a) every WAL entry carries an `event_id` UUID minted at ingest; replay is
   `INSERT ... ON CONFLICT (event_id) DO NOTHING` and a WAL entry is marked consumed only
   after its CRDB txn commits — crash-safe, duplicate-free replay; (b) degraded-mode
   detection is a 2s connect timeout / 5s query timeout, snapshot-vs-WAL ordering is
   WAL-wins (it is strictly newer); (c) total quorum loss is explicitly OUT of scope
   (see Non-goals) — approvals stay fail-closed, and the documented emergency path for
   "must approve a node to rebuild quorum" is Decision 8's audited direct-SQL hatch
   against whatever CRDB survives, or DB-layer restore first.
5. ✅ **uaa-control connects to CRDB with `tokio-postgres` + rustls** — rejected: sqlx
   (macro/offline-cache friction in musl builds), CRDB-specific driver (none maintained).
6. ⚡ **PKI enrollment uses a dedicated install CA — NEVER the CockroachDB CA.** CA
   keypair generated once on the server, `/var/lib/uaa/ca/` mode 0600, loaded only by
   uaa-control; certs minted with `rcgen` (pure Rust). Rejected: reusing the cockroach
   CA, openssl shell-outs. **Repair (ops-judge):** an offline, encrypted CA-key backup
   (same custody as the update key: operator password manager / sealed medium) plus a
   documented restore procedure is a P0 GATE before enrollment ships — a CA with no
   backup is not shippable; a lost CA kills every renewal at the 90-day horizon.
7. ✅ **Agent enrollment flow**: install-CA public cert baked into ISO/PXE seed → agent
   pins it, generates P-256 keypair, submits CSR, POLLS `GetCredential` keyed by SPKI
   fingerprint; operator approves in SPA; control signs and returns cert; agent persists
   key/CSR/claim under `/var/lib/uaa/` and resumes across restarts (idempotent re-claim
   by fingerprint); all subsequent calls are mTLS gRPC. Rejected: TOFU, pre-shared
   tokens. **Repairs from surviving objections:** (a) approving a new enrollment for a
   MAC that already has an `issued` row marks the old row `superseded` (reinstalls wipe
   the state dir and mint a new key — rows must not accrete); (b) renewal (agent
   re-submits same-key CSR at 2/3 lifetime, auto-issue iff an unexpired unrevoked cert
   exists for the SPKI) fail-safe: if control is down through expiry, the agent drops
   from mTLS but the legacy :25000 plane still works; the agent keeps polling and
   re-enters pending (operator re-approve) after expiry.
8. ⚡ **Operator auth = GitHub OAuth web flow ONLY; RBAC from org team membership**
   (`uaa-admins` → admin, `uaa-operators` → operator, org member → viewer; cached 5 min;
   fail-closed to viewer on GitHub API failure). Rejected: local accounts, WebAuthn,
   break-glass, PIV-mTLS (brief-locked). **Repair (ops-judge, without building new
   auth):** the sanctioned emergency path during a GitHub outage is documented, not
   built: an operator with server access performs a direct, logged SQL mutation
   (`cockroach sql`) and MUST backfill an audit event (`uaa-control audit backfill`)
   afterwards; total-lockout is thereby an accepted-and-mitigated risk.
9. ⚡ **Signed self-update, no public phone-home**: each binary embeds its version and an
   ed25519 public key; manifest served by uaa-web; verify manifest sig → compare version
   → download → verify sha256 + artifact sig → `<bin>.new` → atomic rename → restart.
   GitHub-releases fallback opt-in only. **Repairs:** (a) the auto-update TIMER runs
   only on fleet agents/CLI; the three server daemons check-and-STAGE but apply only on
   explicit operator command (`uaa-* self-update --apply`) — this removes the
   two-deploy-paths collision (timer vs sudo-install) the simplicity judge flagged: the
   signed updater IS the deploy path, invoked deliberately on the server; (b) fleet
   rollback is manifest-revert-first (revert uaa-web's manifest, hosts converge), plus a
   per-host `--hold` pin suppressing the timer; `--rollback` (swap `<bin>.prev`) requires
   the manifest revert first or it re-pulls next tick; (c) the manifest carries a
   `min_version` floor so a replayed old-but-signed manifest cannot downgrade.
10. ⚡ **Update-signing + CA key custody**: ed25519 update keypair, private half OFFLINE
    in the operator password manager, public half embedded at build (`UAA_UPDATE_PUBKEY`
    env; `.pub` committed). Signing on the operator machine (`uaa release sign`).
    Rejected: CI-held key. **Repair (compromise recovery):** binaries embed TWO pubkey
    slots (current + next) so rotation stages through the update channel before the old
    key retires; the catastrophic path (both keys burned) is out-of-band manual redeploy,
    documented in the runbook alongside the Decision-6 CA restore.
11. 🔒 **Discovery = mDNS (`_uaa._tcp.local`, TXT: service/version/port) with static
    fallback** (`/etc/uaa/endpoints.yaml`) — the simplicity judge would delete mDNS
    entirely at this fleet size (static file is mandatory anyway for the off-segment
    Tang RPis); the owner's brief locks mDNS-from-scratch. **Repairs adopted:**
    (a) `resolve()` returns the UNION of mDNS and static candidates and callers try each
    under mTLS, accepting the first that authenticates to the expected identity —
    fallback is per-endpoint-failure, never only-on-empty-browse (a stale advertisement
    must not mask a valid static entry); (b) `advertise()` exists only in the three
    daemons — the client-only `uaa` CLI ships browse-only. Crate: `mdns-sd` (pure Rust).
12. ✅ **Machine-plane parity lives in uaa-control** (binds :25000 with exact Python
    paths/status codes; seed READS come straight from the webroot — same host — while
    webroot WRITES stay exclusively in uaa-web). Rejected: parity in uaa-web, dedicated
    uaa-installd. **Normative parity subtleties (correctness judge):** the four seed
    files `/autoinstall/{user-data,meta-data,vendor-data,network-config}` return
    **empty 200** when the hexmac dir exists but the file is missing
    (`autoinstall-agent.py:512`); `/autoinstall/uaa-config` returns **hard 404** on the
    same condition (`:544-548`); unknown MAC/neighbor → 404 for both. The parity matrix
    in the implementation plan is normative and encodes this split per endpoint. The
    :25000 read plane must keep serving while CRDB is degraded (snapshot reads).
13. ⚡ **Boot-target is ONE authoritative registry field** (`machines.boot_target`:
    `local-disk | custom-autoinstall | pxe-disabled | pxe-grub`), and the two mechanisms
    — uaa-web's iPXE `set menu-default` rewrite and uaa-pxe's dnsmasq per-host boot
    program — are PROJECTIONS of it, reconciled by control (the correctness judge showed
    two free-running layers silently no-op a reinstall). `ReinstallMachine` and the
    approve-SAGA set both layers or refuse; `GetMachine` reports the effective target per
    layer plus a `consistent: bool`. **dnsmasq mechanism repair (ops-judge, mechanical):**
    per-host boot config is placed via `dhcp-hostsdir`/`dhcp-optsdir` files — the
    directories dnsmasq actually re-reads on SIGHUP — NOT `/etc/dnsmasq.d/*.conf`
    (conf-dir files are only read at startup; test-then-reload of a conf.d file silently
    no-ops). Gate stays `dnsmasq --test` before reload, and the health probe verifies the
    resolved boot target after reload rather than assuming it took.
14. ✅ **uaa-luks-keys ships as `uaa luks` subcommands** in the agent binary (FIDO2 ops
    run where the YubiKey is plugged; `uaa` is already there). Rejected: standalone
    binary. **Scope note (ops-judge):** the t=2-of-3 guard protects the host being
    rotated; Tang SERVER key rotation additionally requires a fleet re-bind sweep
    (re-enroll SSS bindings on every host that included that Tang) BEFORE the old Tang
    key retires — the `uaa luks rotate-tang` orchestration owns that sweep.
15. ✅ **Power stays a uaa-core library + `uaa power` CLI; uaa-control links the
    library.** ipmitool via `ssh 172.16.2.30`, never locally. Rejected: uaa-power daemon.
    Known residual: if the server itself is down, the power path is down — accepted (the
    server is the constellation host anyway).
16. ⚡ **Python cutover = parity-then-switch with a quiesced import** — rejected:
    side-by-side dual-write. **Repaired sequence (both judges):** (1) stop/drain the
    Python unit FIRST, (2) `uaa-control import --from /var/log/cockroach-autoinstall/`
    (insert-if-absent, Decision 22), (3) start uaa-control on :25000 — no live-writer
    gap, nothing written to JSON after the snapshot. **Rollback is export-first:** any
    re-enable of Python within the ≥2-week window MUST be preceded by
    `uaa-control export --to-json /var/log/cockroach-autoinstall/` (re-hydrating the
    JSON from CRDB); the frozen JSON is authoritative only at t=cutover, never later.
    Roll-forward remains the preferred posture.
17. ✅ **Workspace layout**: cargo workspace — `crates/uaa-core` (library: executor/
    render/verify/place/power/config/luks/discovery/update/pki-client + new `fleet`
    config), `crates/uaa-proto` (protox build of `proto/uaa/**`), `crates/uaa` (CLI+agent),
    `crates/uaa-control`, `crates/uaa-web`, `crates/uaa-pxe`. Root uses
    `members = ["crates/*"]` (glob — adding a crate never edits the members list) and a
    single `[workspace.dependencies]` table populated up-front. Golden tests + the
    311-test baseline move intact into uaa-core. Rejected: separate repos, feature-flag
    single crate.
18. ✅ **Proto packages** `uaa.control.v1`, `uaa.web.v1`, `uaa.pxe.v1`, `uaa.enroll.v1`,
    `uaa.update.v1` under repo `proto/`; BSR module `buf.build/falkcorp/uaa` publish is
    an OPTIONAL Bucket-3 step (zero external consumers today — the simplicity judge is
    right that it's org-consistency ceremony; builds never touch the network either way).
19. 🔒 **Frontend = React + Vite + TypeScript, embedded via `rust-embed`** (shipped
    artifact stays one static binary) — the simplicity judge's dominating alternative is
    NO SPA (server-rendered askama/maud + htmx, no Node toolchain at all), and it is
    recorded here as the strongest rejected option; the owner's brief locks a Node-built
    SPA served by the central Rust server. SvelteKit rejected (equivalent JS surface,
    less org familiarity). `web/dist` is CI-built, never hand-edited.
20. ⚡ **Boot-artifact HTTP**: uaa-web serves the webroot read-only on :8081 plain HTTP
    (UEFI HTTP-boot/iPXE can't practically TLS); nginx keeps the media site and leaves
    the uaa trust path. **Repaired migration (ops-judge):** boot paths are served on
    BOTH nginx:80 and :8081 through the cutover wave; per-host iPXE URLs flip to :8081
    host-by-host; the nginx boot `location` blocks are removed only after a full PXE
    cycle confirms no host references :80. A pre-cutover port audit of
    :7443/:7444/:7445/:7446/:8081/:8443 on the server (CRDB owns :26257 and its admin UI
    commonly sits on :8080, adjacent to :8081) is a Bucket-3 gate; every daemon also
    fail-closes loudly if its bind is taken.
21. ⚡ **Audit log = hash-chained events in CRDB + signed daily checkpoint.**
    **Repairs (correctness + ops):** (a) chain append is SERIALIZED — `prev_hash` is
    read under `SELECT ... FOR UPDATE` on the tip inside the same transaction as the
    mutation it records (concurrent handlers must not fork the chain); genesis
    `prev_hash` = 32 zero bytes; (b) threat model stated explicitly: the chain +
    on-server audit key defends against a rogue OPERATOR without server root; a
    server-root adversary defeats it — out-of-band checkpoint witnessing (appending the
    daily checkpoint hash to a second host / git) is the P2 hardening, recorded not built.
22. ⚡ **Registry seed import = `uaa-control import --from <dir>`, INSERT-IF-ABSENT
    semantics pinned** (`ON CONFLICT (mac) DO NOTHING`; never clobbers a CRDB row that
    is newer than the JSON source) — both judges showed all-column upsert re-runs would
    de-approve live hosts and null out bound TPM EKs during a rollback-retry cycle.
    Paired with `export --to-json` (Decision 16). Server-local, human-run; rejected:
    HTTP migration endpoint.
23. **(NEW — closes the bootstrap defect) Service identity for the three daemons is
    minted server-locally at install time**: `uaa-control ca issue-service --for
    uaa-web|uaa-pxe|uaa-control` runs as root on the server, writes
    `/var/lib/uaa/certs/<svc>.{key,crt}` (0600, per-service user), no network, no
    operator approval (approving your own co-located daemon through the SPA would be
    absurd — simplicity judge). Agents are the ONLY enrollment-flow clients. Service
    certs are 1y, renewed by re-running the command (documented in the deploy runbook).
24. **(NEW) uaa-control restarts must not drop :25000**: the legacy machine-plane
    listener uses systemd socket activation (`uaa-control.socket`), so self-update
    restarts and crashes queue connections instead of refusing them (ops judge: a daily
    timer restart mid-PXE-fetch is an install failure; with Decision 9's repair the
    server applies updates deliberately, and socket activation closes the residual gap).
25. **(NEW) Certificate revocation is enforced, not cosmetic**: uaa-control publishes a
    signed CRL (regenerated on every revocation, fetched by uaa-web/uaa-pxe every 15
    minutes and cached); mTLS verifiers reject listed certs (fail-closed per listed
    cert), and a CRL stale >24h logs loudly but does not kill the plane (fail-open on
    staleness — availability over a 90-day-max exposure that revocation-by-expiry
    already bounds).

## Constellation topology

| Binary | Runs on | Ports | Owns |
|---|---|---|---|
| `uaa` (CLI/agent) | fleet hosts, server, operator Mac | — (client only; mDNS browse-only) | install phases, luks keyslots, power CLI, iso/image/config tooling, vm-validate, self-update (timer-auto) |
| `uaa-control` | the server (172.16.2.30) | :25000 HTTP via systemd socket activation (legacy machine plane, exact Python parity), :15000 HTTPS (operator JSON+OpenAPI + SPA), :15001 gRPC mTLS (services + enrolled agents), :15002 HTTPS install-CA-pinned JSON (enrollment CSR submit/poll, first-boot checkin) | registry (CRDB) + snapshot/WAL degraded mode, enrollment CA + CRL, RBAC, audit chain, approve-SAGA, one-click reinstall, boot-target reconciliation |
| `uaa-web` | the server | :8081 HTTP (boot artifacts, read-only), :7445 gRPC mTLS | `/var/www/html` writes: seeds, iPXE files, ISOs, casper trees, agent binaries, signed update manifests; ISO build jobs (detached) |
| `uaa-pxe` | the server | :7446 gRPC mTLS | dnsmasq per-host boot config via dhcp-hostsdir/optsdir + test-then-reload + post-reload verification, dnsmasq/tftpd health, discovery inbox, optional DNS A/PTR |

All four are `x86_64-unknown-linux-musl` static binaries (arm64 variant is P2). Daemons
advertise on mDNS and load `/etc/uaa/endpoints.yaml`; every binary embeds version +
dual update pubkeys. systemd units (`uaa-control.service` + `.socket`,
`uaa-web.service`, `uaa-pxe.service`) deploy via the signed updater invoked deliberately
(`--apply`), first install by human `sudo install` — server writes stay out of scope for
this plan.

## Data model

### CRDB schema (database `uaa`, owned by uaa-control; migrations embedded, versioned)

```sql
CREATE TABLE machines (
  mac            STRING PRIMARY KEY,            -- normalized aa:bb:cc:dd:ee:ff
  hostname       STRING NOT NULL,
  ip             STRING,
  type           STRING NOT NULL DEFAULT 'lenovo',
  status         STRING NOT NULL DEFAULT 'pending',  -- pending|approved|revoked
  boot_target    STRING NOT NULL DEFAULT 'local-disk',
                 -- authoritative next-boot intent (Decision 13):
                 -- local-disk|custom-autoinstall|pxe-disabled|pxe-grub
  tpm_ek         STRING,                        -- sha256 of TPM EK pub, bound at first checkin
  registered_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
  approved_at    TIMESTAMPTZ,
  last_seen      TIMESTAMPTZ,
  last_ip        STRING,
  installed_at   TIMESTAMPTZ,                   -- parity: persist install completion
  last_install_status STRING,                   -- success|failed|in-progress
  updated_at     TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE TABLE install_history (
  event_id UUID PRIMARY KEY,                    -- minted at INGEST (WAL-replay dedup key)
  mac STRING NOT NULL REFERENCES machines (mac),
  started_at TIMESTAMPTZ, finished_at TIMESTAMPTZ,
  status STRING NOT NULL, detail JSONB
);
CREATE TABLE enrollments (
  spki_fingerprint STRING PRIMARY KEY,           -- sha256 of CSR public key
  mac STRING REFERENCES machines (mac),
  csr_pem STRING NOT NULL,
  state STRING NOT NULL DEFAULT 'pending',       -- pending|approved|issued|rejected|revoked|superseded
  cert_pem STRING, requested_at TIMESTAMPTZ NOT NULL DEFAULT now(), decided_by STRING
);
CREATE TABLE yubikeys (                          -- extends today's GPG/SSH registry
  fingerprint STRING PRIMARY KEY, gpg_pubkey STRING, ssh_pubkey STRING,
  comment STRING, serial STRING, status STRING NOT NULL DEFAULT 'pending',
  registered_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE TABLE luks_credentials (                  -- NEW: FIDO2 keyslot tracking
  id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  mac STRING NOT NULL REFERENCES machines (mac),
  yubikey_serial STRING NOT NULL,
  role STRING NOT NULL,                          -- primary|backup1|backup2
  luks_keyslot INT, enrolled_at TIMESTAMPTZ, revoked_at TIMESTAMPTZ
);
CREATE TABLE tang_servers (
  hostname STRING PRIMARY KEY, ip STRING, tang_url STRING,
  adv_keys JSONB, last_seen TIMESTAMPTZ
);
CREATE TABLE discovered_macs (                   -- uaa-pxe inbox
  mac STRING PRIMARY KEY, first_seen TIMESTAMPTZ, last_seen TIMESTAMPTZ,
  arch_hint STRING, vendor_class STRING, dismissed BOOL NOT NULL DEFAULT false
);
CREATE TABLE audit_events (
  seq INT8 PRIMARY KEY DEFAULT unique_rowid(),
  at TIMESTAMPTZ NOT NULL DEFAULT now(),
  actor STRING NOT NULL, role STRING NOT NULL,   -- github login / 'system'
  action STRING NOT NULL, target STRING, outcome STRING NOT NULL,
  detail JSONB, prev_hash BYTES NOT NULL, hash BYTES NOT NULL
  -- append serialized via SELECT tip FOR UPDATE in the recording txn (Decision 21);
  -- genesis prev_hash = 32 zero bytes
);
CREATE TABLE audit_checkpoints (
  day DATE PRIMARY KEY, tip_seq INT8 NOT NULL, tip_hash BYTES NOT NULL,
  signature BYTES NOT NULL                       -- ed25519, on-server audit key
);
CREATE TABLE saga_log (
  saga_id UUID PRIMARY KEY, kind STRING NOT NULL,
  state STRING NOT NULL,  -- running|done|compensating|compensated|compensation_pending
  steps JSONB NOT NULL, started_at TIMESTAMPTZ, finished_at TIMESTAMPTZ
);
```

### Local fallback (uaa-control, Decision 4)

`/var/lib/uaa/registry-snapshot.json` rewritten (tmp+rename, 0600) after every successful
mutation; on CRDB unavailability (2s connect / 5s query timeout) control serves reads
from the snapshot, 503s mutations, and appends telemetry to `/var/lib/uaa/wal.jsonl`
(each entry: `event_id` UUID + payload). Replay on reconnect:
`INSERT ... ON CONFLICT (event_id) DO NOTHING`, entry marked consumed only after commit.
Fail-closed for approvals; fail-open for telemetry ingestion. Does NOT cover quorum loss
(Non-goals).

### Proto surface (package → service → RPCs; full .proto files are a GENERATE artifact)

```proto
// proto/uaa/control/v1/control.proto
service ControlService {
  rpc ApproveMachine(ApproveMachineRequest) returns (ApproveMachineResponse); // SAGA
  rpc GetMachine(GetMachineRequest) returns (Machine);         // incl. per-layer effective boot target + consistent flag
  rpc ListMachines(ListMachinesRequest) returns (ListMachinesResponse);
  rpc RecordInstallEvent(RecordInstallEventRequest) returns (Ack);  // carries client event_id
  rpc ReinstallMachine(ReinstallMachineRequest) returns (ReinstallMachineResponse);
  rpc UpsertDiscoveredMac(UpsertDiscoveredMacRequest) returns (Ack);  // uaa-pxe inbox
}
// proto/uaa/enroll/v1/enroll.proto  (also exposed as JSON on :15002)
service EnrollService {
  rpc SubmitCsr(SubmitCsrRequest) returns (SubmitCsrResponse);       // idempotent by SPKI fp
  rpc GetCredential(GetCredentialRequest) returns (GetCredentialResponse); // poll
}
// proto/uaa/web/v1/web.proto
service WebService {
  rpc PlaceSeed(PlaceSeedRequest) returns (Ack);        // non-secret fields + placeholders ONLY
  rpc PlaceIpxe(PlaceIpxeRequest) returns (Ack);
  rpc FlipBootTarget(FlipBootTargetRequest) returns (FlipBootTargetResponse);
  rpc RemoveHost(RemoveHostRequest) returns (Ack);
  rpc ListIsos(ListIsosRequest) returns (ListIsosResponse);
  rpc BuildIso(BuildIsoRequest) returns (BuildIsoResponse);          // detached job id
  rpc GetBuildJob(GetBuildJobRequest) returns (BuildJob);
  rpc PublishAgentBinary(PublishAgentBinaryRequest) returns (Ack);   // verifies detached sig first
}
// proto/uaa/pxe/v1/pxe.proto
service PxeService {
  rpc SetupPxe(SetupPxeRequest) returns (Ack);          // per-host dhcp-hostsdir/optsdir files + verified reload
  rpc SetBootTarget(SetBootTargetRequest) returns (Ack);// projection of machines.boot_target
  rpc Health(HealthRequest) returns (HealthResponse);   // dnsmasq/tftpd liveness + TFTP probe + boot-target verification
  rpc StreamDiscoveredMacs(StreamDiscoveredMacsRequest) returns (stream DiscoveredMac);
  rpc SetDnsRecord(SetDnsRecordRequest) returns (Ack);  // optional A/PTR (P2)
}
// proto/uaa/update/v1/update.proto — manifest types (served as JSON by uaa-web): name,
// version, target, sha256, sig, url, min_version
```

Conventions: proto3, `*Request`/`*Response` wrappers, no field reuse, versioned packages,
breaking change = new vN package (org gcommon house style).

## Components

### C1. uaa-core (`crates/uaa-core`)

The existing library moves here intact (all 311 tests). New modules:

```rust
// crates/uaa-core/src/discovery.rs
pub struct ServiceInfo { pub service: ServiceKind, pub version: semver::Version,
                         pub host: IpAddr, pub port: u16, pub source: Source /* Mdns|Static */ }
pub async fn advertise(info: &ServiceInfo) -> Result<DiscoveryHandle>;    // daemons only
pub async fn resolve(kind: ServiceKind, static_fallback: &EndpointsFile,
                     timeout: Duration) -> Result<Vec<ServiceInfo>>;
// UNION of mDNS + static, mDNS-sourced first; callers iterate candidates under mTLS and
// accept the first that authenticates (Decision 11 repair). Empty union = hard error.
// crates/uaa-core/src/update.rs
pub struct Manifest { pub binaries: Vec<BinaryEntry>, pub min_version: semver::Version }
pub async fn self_update(current: &BinaryIdentity, manifest_url: &str,
                         pubkeys: &[VerifyingKey; 2],   // current + next (Decision 10)
                         mode: ApplyMode /* TimerAuto | StageOnly */) -> Result<UpdateOutcome>;
// crates/uaa-core/src/pki.rs (client side)
pub fn generate_keypair_and_csr(identity: &AgentIdentity) -> Result<(KeyPem, CsrPem)>;
pub async fn enroll_poll(endpoint: &Url, pinned_ca: &CertPem, state_dir: &Path)
    -> Result<Credential>;   // persists across restarts; re-claim by SPKI fingerprint
// crates/uaa-core/src/fleet.rs
pub struct FleetConfig { /* netboot server, ports, tang urls, host registry, deny-list */ }
pub fn load_or_default() -> FleetConfig;  // /etc/uaa/fleet.yaml; defaults = today's constants
```

Fleet constants (`172.16.2.30`, `:25000`, Tang URLs, `lookup_host`, the `unimatrixone`
power deny-list) move behind `FleetConfig` with current values as defaults —
behavior-preserving (existing tests pass on defaults). Fail-closed: `resolve()` with an
empty candidate union is an error, never a guess.

### C2. uaa-proto (`crates/uaa-proto`)

`build.rs` runs protox over `proto/uaa/**` (no protoc, no network); generated code not
committed. Exposes tonic clients + servers per package. musl gate:
`cargo build --offline --target x86_64-unknown-linux-musl` stays green.

### C3. uaa-control (`crates/uaa-control`)

- **Machine plane (:25000, axum, socket-activated)** — endpoint-for-endpoint parity with
  `scripts/autoinstall-agent.py` (25 endpoints; parity matrix normative in the
  implementation plan). Semantics preserved exactly, including: empty-200 for a missing
  seed FILE under an existing hexmac dir vs hard-404 for missing `uaa.yaml`
  (Decision 12); hard-404 unknown single resources; empty-200 collections; webhook flip
  tolerance for missing iPXE files (USB-only hosts); TPM-EK first-bind + mismatch-403;
  `ip neigh` MAC resolution (legacy trust, dies with this port). Read paths keep serving
  under CRDB degradation (snapshot).
- **Enrollment plane (:15002, HTTPS server-auth)** — JSON mirror of `EnrollService`.
  Fail-closed: unknown SPKI polls get 404, never auto-issue.
- **Operator plane (:15000)** — axum + utoipa (`/api/openapi.json`), GitHub OAuth →
  signed session cookie, RBAC middleware (fail-closed to viewer on GitHub failure —
  mutations denied, reads allowed), SPA via rust-embed.
- **Approve SAGA** (`ApproveMachine`), ORDERED not parallel (ops-judge defect repair):
  (1) uaa-web.PlaceSeed + PlaceIpxe (inert placement first), (2) uaa-pxe.SetupPxe +
  SetBootTarget (activation LAST — a failure between steps leaves the host inert, never
  activated-with-no-seed), (3) registry status=approved + boot_target write + audit.
  Compensation runs the reverse; an unreachable participant parks the saga in
  `compensation_pending` with exponential retry — it is NEVER falsely marked
  `compensated`. Resumable from `saga_log` after restart.
- **One-click reinstall** (`ReinstallMachine`): set boot_target=custom-autoinstall →
  project to BOTH layers (uaa-web flip + uaa-pxe target; refuse if either layer cannot
  be reconciled, Decision 13) → power cycle via uaa-core power lib → watch install
  events until success/timeout. **Bounded (ops-judge):** on watch timeout, attempt
  fail-safe flip-back to local-disk + alert; a reinstall counter refuses a re-trigger
  within a cooldown unless the operator confirms. Hard rules: refuses `unimatrixone`
  (FleetConfig deny-list) and any non-approved host.
- **Audit**: every mutating handler calls `audit::record(...)`; the append runs in the
  same CRDB txn as the mutation with `SELECT tip FOR UPDATE` serialization (Decision 21);
  daily signed checkpoint row.
- **CA + CRL** (Decision 6/23/25): rcgen signing; `ca issue-service` server-local
  command; CRL regenerated on revocation, served to the other daemons; offline CA backup
  runbook is a P0 ship-gate.

### C4. uaa-web (`crates/uaa-web`)

Owns every write under `/var/www/html`: `PlaceSeed` (typed placeholder gate — payload
may contain ONLY non-secret fields and `REPLACE_AT_PLACE_TIME` placeholders; any real
secret is rejected fail-closed), `PlaceIpxe`, `FlipBootTarget` (same `set menu-default`
regex rewrite as today's `flip_ipxe()`; missing file → `ok:false`, not an error),
`RemoveHost`, ISO inventory/build (`BuildIso` = detached tokio job wrapping the
tooling-port pipeline, never inline), `PublishAgentBinary` (verifies the artifact's
detached sig before placement; uaa-web never holds a signing key) + manifest
regeneration. Serves the webroot read-only on :8081 (tower-http ServeDir behind an
explicit path allowlist mirroring today's nginx locations). All writes atomic
(tmp+rename). Runs under its service cert (Decision 23).

### C5. uaa-pxe (`crates/uaa-pxe`)

Projects per-host boot config into `dhcp-hostsdir`/`dhcp-optsdir` files (Decision 13 —
NOT conf.d), gates with `dnsmasq --test`, reloads, then VERIFIES the applied target via
its health probe. Health: dnsmasq + tftpd-hpa unit state + TFTP self-probe + boot-target
consistency. Discovery inbox: follows the dnsmasq journal for DHCP/proxy-DHCP requests
from MACs absent from the registry; upserts via `ControlService.UpsertDiscoveredMac`;
streams to the SPA queue. DNS A/PTR (P2) uses a dedicated hosts file, same gate.

### C6. Enrollment PKI state machine

```
agent boot ──▶ load /var/lib/uaa/{agent.key,agent.csr,claim.json}   (create if absent)
   │  pin install-ca.crt (from seed/ISO)             (no CA file → abort + retry loop, fail-closed)
   ├─ SubmitCsr (idempotent upsert by SPKI fp)
   ├─ GetCredential poll (backoff 30s→5m cap) ──▶ pending: keep polling (survives reboot)
   │                                          └▶ issued: persist agent.crt → mTLS gRPC :15001
   └─ rejected/revoked: log loudly, hold at 1h poll (operator can re-approve)
Approve (SPA): pending CSR list (SPKI fp + claimed MAC/hostname + discovery-inbox
correlation) → approve/reject → control signs (rcgen, 90d, SAN = hostname + mac URI);
approving a fp for a MAC with an existing issued row marks that row `superseded`.
Renewal: same-key CSR at 2/3 lifetime; auto-issue iff unexpired+unrevoked cert exists for
the SPKI; expired-through-outage agents fall back to pending (re-approve) — legacy :25000
keeps working meanwhile. Revocation: CRL per Decision 25.
Service daemons NEVER use this flow — Decision 23.
```

### C7. Signed self-update

Manifest `http://<uaa-web>:8081/uaa/manifest.json` + detached `.sig` (ed25519 over
manifest bytes): entries `{name, version, target, sha256, sig, url}` + global
`min_version`. Verify order (fail-closed at every step): manifest sig → version newer
AND ≥ min_version → download → sha256 → artifact sig → `<bin>.new` → rename → restart
(daemons: only on `--apply`, socket activation holds :25000; agents/CLI: timer).
Rollback: manifest-revert FIRST, then per-host `<bin>.prev` swap; `--hold` pins a host.

### C8. `uaa luks` (FIDO2+PIN keyslot manager, NOT auth)

`uaa luks enroll --role primary|backup1|backup2` (wraps `systemd-cryptenroll
--fido2-device=auto --fido2-with-client-pin=yes`; touch at creation unavoidable),
`status` (parses `cryptsetup luksDump` fido2 tokens — reuses `evaluate_fido2_keyslot`),
`rotate` (enroll-new-then-revoke-old, never reverse), `revoke --serial`, and
`rotate-tang` (fleet-aware re-bind sweep, Decision 14). Registry sync to
`luks_credentials` (3-credential-per-host model per `PLAN-zfs-luks-multikey.md`).
**Cold-start guard:** any rotation verifies ≥2 of 3 Tang servers remain valid for every
affected binding BEFORE removing anything — fail-closed; override requires typing the
hostname.

## Migration / integration (parity-then-switch)

1. **Foundation** — workspace conversion (behavior-frozen), proto crate, core modules,
   per-binary musl matrix.
2. **Parity** — uaa-control machine plane passes the parity fixture suite + full
   VM-harness install loop. Degraded-mode registry layer ships HERE (it is part of the
   registry foundation, not an afterthought — judge defect).
3. **Security** — enrollment PKI + service-cert bootstrap + CRL + OAuth/RBAC + audit
   chain (new ports, additive only). CA backup runbook gates this milestone.
4. **Features** — SAGA approve, discovery inbox, one-click reinstall, ISO self-service,
   luks manager, power finish, self-update.
5. **E2E VM gate** — enroll → approve → cert → install → verify sweep inside QEMU+swtpm.
6. **Cutover & retire** (operational, Bucket 3 except code deletions): port audit →
   quiesce Python → import → start Rust on :25000 → dual-serve boot paths :80+:8081 →
   flip iPXE URLs → drain → remove nginx boot locations → ≥2-week window (rollback =
   export-to-JSON first) → delete Python + each ported shell script, each deletion gated
   on its replacement's gate.

Hard rules (restated in every relevant brief): NO hardware actions; validate only via
cargo + `scripts/vm-validate.sh`; NEVER wipe/write 172.16.2.30 or len-serv-003 until
VM-validated; `disk_device` read from the live target, never guessed; ipmitool via
`ssh 172.16.2.30`; NEVER power on unimatrixone; no real secret committed
(`REPLACE_AT_PLACE_TIME` stays a placeholder); version headers bumped; workers never
push/PR/merge.

## Milestones

- **M1 — Workspace + proto foundation.** Additive-by-construction (CLI behavior frozen).
- **M2 — Machine-plane parity + registry (incl. degraded mode) in VM.** Gate: parity
  fixtures + vm-validate loop against uaa-control.
- **M3 — Enrollment + service identity + operator plane.** New ports only; CA-backup
  runbook is the ship-gate.
- **M4 — Constellation features.** SAGA/discovery/reinstall/ISO/luks/power/update.
- **M5 — E2E VM gate.** THE gate before any hardware.
- **M6 — Cutover + retirement.** The one behavior-changing milestone; quiesce-import-swap
  with export-first rollback held ≥2 weeks.

## Files modified

| Area | Change |
|---|---|
| `Cargo.toml`, `src/**` → `crates/**` | workspace conversion (M1, single ⚠ task) |
| `proto/uaa/**`, `crates/uaa-proto` | NEW |
| `crates/uaa-control`, `crates/uaa-web`, `crates/uaa-pxe` | NEW |
| `crates/uaa-core/src/{discovery,update,pki,fleet}.rs` | NEW modules |
| `web/` (React+Vite SPA) | NEW |
| `.github/workflows/musl-build.yml` | per-binary matrix |
| `scripts/{autoinstall-agent.py,make-ssh-ready-iso.sh,deploy-usb-configs.sh,build-installer-image.sh}` | retired at M6, each gated |

## Testing

| Test | Asserts |
|---|---|
| parity fixture suite (uaa-control) | status/body parity with Python handlers incl. empty-200-missing-seed-file vs 404-missing-uaa-config, 403 conventions, TPM bind |
| degraded-mode tests | snapshot reads under CRDB loss; WAL replay is duplicate-free (ON CONFLICT event_id); approvals 503 |
| enrollment state-machine tests | idempotent re-claim, restart resume, supersede-on-reinstall, reject/revoke holds, renewal same-key path, expiry-through-outage → pending |
| SAGA tests | ordered placement-then-activation; compensation on each step; unreachable participant → compensation_pending, never falsely compensated; resume from saga_log |
| boot-target reconciliation tests | both layers projected; inconsistency detected and reported; reinstall refuses unreconcilable state |
| discovery tests | union of mDNS+static; per-endpoint-failure fallback; empty union = error |
| update tests | bad sig/sha/min_version rejected; atomic rename; hold pin; manifest-revert convergence; dual-pubkey rotation |
| audit chain tests | concurrent appends never fork (serialized tip); tamper breaks verification; genesis fixed |
| CRL tests | listed cert rejected; stale CRL logs but serves |
| VM e2e (extended vm-validate) | enroll→approve→cert→install→19-check verify sweep in QEMU+swtpm |

## Rollback

M1–M5 are additive (new binaries/ports; :25000 stays Python) — rollback = don't deploy.
M6 rollback = `uaa-control export --to-json` THEN re-enable `autoinstall-agent.service`
(kept disabled-but-present ≥2 weeks); the frozen JSON is authoritative only at t=cutover.
Self-update rollback = manifest revert first, then `<bin>.prev`. No schema destructions
anywhere in v1.

## Open questions (resolved — recorded for the plan)

1. ~~Registry store~~ → CockroachDB w/ snapshot+WAL degraded mode; quorum loss out of
   scope (Decision 4).
2. ~~Browser API~~ → JSON+OpenAPI; GraphQL named alternative (Decision 3).
3. ~~Frontend~~ → React+Vite (owner-locked; no-SPA/htmx recorded as strongest rejected
   alternative) (Decision 19).
4. ~~mDNS vs static off-segment~~ → union resolution, per-failure fallback; owner-locked
   over static-only (Decision 11).
5. ~~luks-keys packaging~~ → `uaa luks` subcommand (Decision 14).
6. ~~Workspace/proto/BSR layout~~ → Decisions 17/18 (BSR publish optional Bucket 3).
7. ~~Key custody~~ → offline update key + dual-pubkey rotation + CA backup gate
   (Decisions 6/10).
8. ~~Python cutover~~ → quiesce-import-swap, export-first rollback (Decision 16).
