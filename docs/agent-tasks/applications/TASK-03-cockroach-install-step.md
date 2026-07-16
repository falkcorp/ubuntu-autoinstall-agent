<!-- file: docs/agent-tasks/applications/TASK-03-cockroach-install-step.md -->
<!-- version: 1.0.0 -->
<!-- guid: 749076f4-41ed-4df2-9ad5-66c49c47221b -->
<!-- last-edited: 2026-07-16 -->

# TASK-03 — Cockroach install step: port `setup_cockroachdb.sh` into Phase 5 (DS-APP-03)

**Priority:** P1 · **Effort:** L · **Recommended subagent:** Sonnet-class · rust-core subagent · **Why:** ports an out-of-git shell script into a chroot-executed Rust step — systemd unit, cert fetch, and join derivation must each be exact. · **Depends on:** DS-APP-02 (fills its `install_cockroach` stub)

## ⛔ START HERE (do this first, exactly)

```bash
REPO=/Users/jdfalk/repos/github.com/jdfalk/ubuntu-autoinstall-agent   # adjust to your clone
git -C "$REPO" fetch origin
git -C "$REPO" worktree add "$REPO/.worktrees/applications-cockroach-install-step" -b agent/applications-cockroach-install-step origin/main
cd "$REPO/.worktrees/applications-cockroach-install-step"
git rebase origin/main
```

(Protocol also in `docs/agent-tasks/ORCHESTRATION.md` — the inline block above is authoritative for this task.)

**Wave gate:** DS-APP-02 must be merged. If `grep -n "async fn install_cockroach" crates/uaa-core/src/network/ssh_installer/applications.rs` returns 0 hits, the gate is not met: STOP and report.

## Goal

Replace DS-APP-02's `install_cockroach` stub (which currently returns a `ConfigError` naming DS-APP-03) with a real implementation that installs and starts a CockroachDB node inside the target.

**You do not need the netboot server.** The source script is **not in git** — it lives only at `172.16.2.30:/var/www/html/cloud-init/scripts/setup_cockroachdb.sh` and `rm -- "$0"`s itself after running. It was retrieved 2026-07-16 and is reproduced **verbatim below** as your reference. Do not try to fetch it.

REUSE — do not invent parallels:

- **`HostSpec::compute_join`** for the join string — verify: `grep -n "pub fn compute_join" crates/uaa-core/src/autoinstall/host_spec.rs`. It puts the seed first, then members excluding self, preserving order. **Do NOT write a second join implementation** — a divergence between the two would be invisible until a node failed to join.
- **`HostSpec::ip_without_cidr`** — verify: `grep -n "pub fn ip_without_cidr" crates/uaa-core/src/autoinstall/host_spec.rs`. **Mandatory**, see the trap below.
- **The chroot shape** `chroot /mnt/targetos bash -lc '<cmd>'` — verify: `grep -c "chroot /mnt/targetos bash -lc" crates/uaa-core/src/network/ssh_installer/system_setup.rs` (~21 hits). Copy it; do not invent another.
- **`crate::error::AutoInstallError`** for errors. No new error enum, no new dependency.

## Background (verify before editing)

### The source script (verbatim, retrieved 2026-07-16 — NOT in git)

```bash
#!/bin/bash
set -e
source /root/variables.sh

# Download CockroachDB v25.3.0 (arch-aware)
ARCH=$(uname -m)
if [ "$ARCH" = "aarch64" ] || [ "$ARCH" = "arm64" ]; then
    CRDB_ARCH="linux-arm64"
else
    CRDB_ARCH="linux-amd64"
fi
curl -sSfL "https://binaries.cockroachdb.com/cockroach-v25.3.0.${CRDB_ARCH}.tgz" | tar xz -C /tmp
cp -f "/tmp/cockroach-v25.3.0.${CRDB_ARCH}/cockroach" /usr/local/bin/
rm -rf "/tmp/cockroach-v25.3.0.${CRDB_ARCH}"

# Create user and directories
useradd -r -m -d /var/lib/cockroach cockroach 2>/dev/null || true
mkdir -p /var/lib/cockroach/certs /var/lib/cockroach/data
chown -R cockroach:cockroach /var/lib/cockroach

# Get node cert from autoinstall-agent
SOURCE_IP=$(ip route get 1 | awk '{print $7; exit}')
HOSTNAME_VAL=$(hostname)
CERT_JSON=$(curl -fsSL "http://172.16.2.30:25000/api/certs/${HOSTNAME_VAL}?ip=${SOURCE_IP}")
echo "$CERT_JSON" | python3 -c "
import sys, json, base64, os
data = json.load(sys.stdin)
if not data.get('ok'):
    print('Failed to get certs:', data.get('error'), file=sys.stderr)
    sys.exit(1)
certs_dir = '/var/lib/cockroach/certs'
for fname, b64content in data['certs'].items():
    path = os.path.join(certs_dir, fname)
    with open(path, 'wb') as f:
        f.write(base64.b64decode(b64content))
"
chown cockroach:cockroach /var/lib/cockroach/certs/*
chmod 644 /var/lib/cockroach/certs/ca.crt /var/lib/cockroach/certs/node.crt
chmod 600 /var/lib/cockroach/certs/node.key

# Derive SQL port from RPC port (36357 -> 36257)
RPC_PORT=$(echo $COCKROACH_ADVERTISE | cut -d: -f2)
SQL_PORT=$(echo $RPC_PORT | sed 's/36357/36257/')
SQL_ADDR="${SOURCE_IP}:${SQL_PORT}"

cat > /etc/systemd/system/cockroach.service << SVCEOF
[Unit]
Description=CockroachDB
After=network-online.target
[Service]
User=cockroach
ExecStart=/usr/local/bin/cockroach start \
  --store=/var/lib/cockroach/data \
  --certs-dir=/var/lib/cockroach/certs \
  --listen-addr=${COCKROACH_ADVERTISE} \
  --advertise-addr=${COCKROACH_ADVERTISE} \
  --sql-addr=${SQL_ADDR} \
  --join=${COCKROACH_JOIN} \
  --cache=${COCKROACH_CACHE} \
  --max-sql-memory=${COCKROACH_MAX_SQL} \
  --locality=${COCKROACH_LOCALITY} \
  --http-addr=:38080
Restart=always
RestartSec=10s
LimitNOFILE=500000
[Install]
WantedBy=multi-user.target
SVCEOF

systemctl daemon-reload
systemctl enable cockroach
systemctl start cockroach
rm -- "$0"
```

Porting this into Rust also **removes a plain-HTTP fetch-and-exec of an ungoverned, self-deleting script from first boot** — that is a real security improvement, not just a refactor.

### What changes in the port

- **Values come from `CockroachSpec` + derivation, not `/root/variables.sh`.** `version`, `port`, `sql_port`, `http_addr`, `seed_ip`, `cache`, `max_sql_memory`, `locality` are `CockroachSpec` fields (DS-APP-01). `advertise` and `join` are **derived**.
- **⚠ Strip the CIDR — this is the trap.** `config.network_address` is `172.16.3.92/23`. `compute_join` takes **bare IPs** and filters self **by IP**. Feeding it unstripped yields `172.16.3.92/23:36357` and a self-filter that never matches — **the node would list itself in its own join string**. Apply `HostSpec::ip_without_cidr` to self and to every member.
- **⚠ Exclude soft-released members** from the member list. A decommissioned node must not stay in every new node's join string.
- **`sql_port` is a field, not a `sed`.** The script's `sed 's/36357/36257/'` is a hack that silently no-ops for any other port. Use `spec.sql_port`.
- **Cert fetch stays** — `/api/certs/<hostname>?ip=<ip>` is an **existing** endpoint (verify: `grep -n "/api/certs/:hostname" crates/uaa-control/src/machine_plane/inventory.rs`). Fetch it inside the chroot with `curl`, exactly as the script does; the install CA trust anchor is already in place because DS-APP-02 put this step **after** `install_ca_cert_in_chroot`.
- **Drop the `python3` dependency.** The script shells to python3 to base64-decode the cert JSON. Use `curl` + a small `bash`/`jq`-free decode, or parse the JSON in Rust and write the certs via separate chroot commands. Do NOT add python3 to the target package set.
- **Drop `rm -- "$0"`** — there is no script to delete.
- Edge semantics (spelled out here AND in acceptance):
  - **Cert fetch returns `ok: false` or non-2xx** → hard error naming the endpoint and the body. Never proceed to write a unit that would fail to start.
  - **`seed_ip` equals this host's IP** (this node IS the seed) → legal; `compute_join` already puts the seed first, and a seed joining itself is how a new cluster starts.
  - **Zero sibling members** → legal; the join string is just the seed.
  - **Any command failing** → propagate with `?`. DS-APP-02 made this step fail-closed; do not add a `warn!`-and-continue.

**HARD RULES (non-negotiable):**
- NO hardware actions. Every command goes through the `CommandExecutor` mock in tests. NEVER run this against a real host.
- NEVER wipe/write/deploy on 172.16.2.30 or len-serv-003. NEVER power on unimatrixone.
- **Do NOT fetch anything from 172.16.2.30 while implementing** — the script is reproduced above; that is your only source.
- No real secret in any file; `REPLACE_AT_PLACE_TIME` stays a placeholder.
- Do NOT add `python3`, `jq`, or any package to the target apt list without it being strictly required — and if you think it is, say so in your report rather than adding it silently.
- Stay inside your worktree; never `git push`, `gh pr`, or merge — report done and stop.

- **Re-verify these anchors before editing** — line numbers drift; zero hits = STOP and report:
  ```bash
  grep -n "async fn install_cockroach" crates/uaa-core/src/network/ssh_installer/applications.rs
  # expect: 1 hit — DS-APP-02's stub (0 hits = wave gate not met, STOP)
  grep -n "pub fn compute_join" crates/uaa-core/src/autoinstall/host_spec.rs
  # expect: 1 hit (~line 49) — REUSE; do not write a second join impl
  grep -n "pub fn ip_without_cidr" crates/uaa-core/src/autoinstall/host_spec.rs
  # expect: 1 hit (~line 37) — MANDATORY; network_address carries CIDR
  grep -n "pub const COCKROACH_PORT" crates/uaa-core/src/autoinstall/host_spec.rs
  # expect: 1 hit (~line 13) — 36357; CockroachSpec.port defaults to it
  grep -c "chroot /mnt/targetos bash -lc" crates/uaa-core/src/network/ssh_installer/system_setup.rs
  # expect: ~21 — the chroot shape to copy
  grep -n "install_ca_cert_in_chroot" crates/uaa-core/src/network/ssh_installer/installer.rs
  # expect: 1 hit — proof your step runs AFTER the trust anchor lands
  ```

## Step-by-step

1. Open `crates/uaa-core/src/network/ssh_installer/applications.rs` (DS-APP-02's file). Keep its guid; bump its version.
2. Add a pure, testable derivation helper (unit-testable with no executor):
   ```rust
   /// Build (advertise, join) for this host. `members` are sibling network_address
   /// values (CIDR form) from the group, EXCLUDING soft-released ones.
   /// Strips CIDR from self and every member before calling compute_join —
   /// compute_join filters self BY IP, so an unstripped self never matches and
   /// the node lists itself in its own join string.
   pub fn derive_cockroach_endpoints(
       self_network_address: &str,
       members: &[String],
       spec: &CockroachSpec,
   ) -> (String, String);
   ```
   It must call `HostSpec::ip_without_cidr` and `HostSpec::compute_join` — never reimplement either.
3. Implement `install_cockroach` as an ordered sequence of `chroot /mnt/targetos bash -lc '…'` commands mirroring the script: arch detect → download+install binary → `useradd`/dirs/`chown` → cert fetch + write + perms → write `/etc/systemd/system/cockroach.service` → `daemon-reload` → `enable` → `start`. Each fallible step propagates with `?`.
4. Keep purely additive — do not modify `installer.rs`'s Phase-5 ordering, `config.rs`, or `host_spec.rs`.
5. Add tests in `applications.rs`'s `mod tests` (mock executor; no network):
   - **`test_cockroach_join_matches_host_spec`** — `derive_cockroach_endpoints` output equals `HostSpec::compute_join(seed, members, self_ip, port)` for the real fleet values. *Proves there is no second implementation.*
   - **`test_derive_strips_cidr`** — self `172.16.3.92/23` ⇒ advertise `172.16.3.92:36357`, **never** `172.16.3.92/23:36357`; and the join does **not** contain self.
   - `test_derive_excludes_released_members` — a released member is absent from the join.
   - `test_derive_seed_is_self_is_legal` — this node is the seed ⇒ join contains the seed once, no error.
   - `test_derive_zero_members` — join is just the seed.
   - `test_cockroach_writes_unit_and_starts` — the mock recorded, in order: a `curl` for the binary, a `useradd`, a `/api/certs/` fetch, a write of `cockroach.service`, `daemon-reload`, `enable`, `start`.
   - `test_cert_fetch_failure_propagates` — mock the cert fetch failing ⇒ `Err`, and **no `systemctl start` was recorded** (never start a node whose certs are missing).
   - `test_sql_port_from_spec_not_sed` — a spec with `port: 40000, sql_port: 40001` yields `--sql-addr` on 40001, proving no `sed 36357→36257` hack survived.
6. Bump the header on `applications.rs`; keep its guid.

**Anti-over-suppression:** `test_cockroach_writes_unit_and_starts` is the happy-path guard against `test_cert_fetch_failure_propagates`'s error path over-suppressing — i.e. proving the fail-closed cert check does not reject a *valid* cert response and block every install.

## How to test

```bash
cargo test --lib --offline
# Expected: 639+ passed, 0 failed (baseline + DS-APP-01/02's tests + your 8).
cargo build --offline
# Expected: exit 0.
cargo clippy --offline -- -D warnings
# Expected: no warnings.
```

Note: this task is **not** VM-validated — the QEMU gate assertion is DS-APP-05, which depends on this task and DS-APP-04.

## Acceptance criteria

- [ ] `cargo test --lib --offline` exits 0 — verify: `cargo test --lib --offline 2>&1 | grep -E "^test result"`
- [ ] `cargo build --offline` exits 0 — verify: `cargo build --offline && echo BUILD_OK`
- [ ] **No second join implementation** — verify: `cargo test --lib --offline test_cockroach_join_matches_host_spec`
- [ ] **CIDR is stripped** — verify: `cargo test --lib --offline test_derive_strips_cidr`
- [ ] `ip_without_cidr` and `compute_join` are actually called — verify: `grep -c "ip_without_cidr\|compute_join" crates/uaa-core/src/network/ssh_installer/applications.rs` returns ≥2
- [ ] Fail-closed on a bad cert fetch — verify: `cargo test --lib --offline test_cert_fetch_failure_propagates`
- [ ] Anti-over-suppression: the happy path still installs — verify: `cargo test --lib --offline test_cockroach_writes_unit_and_starts`
- [ ] No `warn!`-and-continue was introduced — verify: `grep -c "warn!" crates/uaa-core/src/network/ssh_installer/applications.rs` returns **0**
- [ ] No python3 added to the target package set — verify: `git diff origin/main -- crates/uaa-core/src/network/ssh_installer/system_setup.rs | grep -c "python3"` returns **0**
- [ ] No direct process spawn — verify: `grep -c "std::process::Command" crates/uaa-core/src/network/ssh_installer/applications.rs` returns **0**
- [ ] `cargo clippy --offline -- -D warnings` clean
- [ ] File header bumped — verify: `grep -n "last-edited: 2026-07" crates/uaa-core/src/network/ssh_installer/applications.rs`

## Commit message

```
feat(installer): install and start a CockroachDB node in Phase 5 (DS-APP-03)

Replaces DS-APP-02's stub with a real port of setup_cockroachdb.sh — a script
that lives only on the netboot server, is fetched over plain HTTP at first
boot, and rm's itself after running. Porting it into a chroot-executed Rust
step removes that ungoverned fetch-and-exec from the boot path.

advertise/join are derived via HostSpec::compute_join rather than a second
implementation, and HostSpec::ip_without_cidr is applied first: network_address
carries CIDR, and compute_join filters self BY IP, so an unstripped self never
matches and the node would list itself in its own join string. Soft-released
members are excluded.

sql_port is a CockroachSpec field, replacing the script's
`sed 's/36357/36257/'`, which silently no-ops for any other port. The python3
base64 hop is dropped rather than adding python3 to the target.

A failed cert fetch fails the install rather than starting a node whose certs
are missing.

Co-Authored-By: Claude <noreply@anthropic.com>
```

## Done

STOP — report done with exact counts; the coordinator owns push/PR/merge.

## Idempotency / Rollback

**Polarity: transform** (replaces the stub's `Err` body with a real implementation). If `grep -c 'cockroach application install not yet implemented' crates/uaa-core/src/network/ssh_installer/applications.rs` returns **0** AND `grep -n "pub fn derive_cockroach_endpoints" crates/uaa-core/src/network/ssh_installer/applications.rs` hits, this task is already applied — run the acceptance checks instead of re-applying. Rollback = revert the single commit, restoring the loud stub; every committed host config has `applications: []` so no machine's install changes either way, and no data or schema is touched. DS-APP-05 depends on this and must rebase after it merges.
