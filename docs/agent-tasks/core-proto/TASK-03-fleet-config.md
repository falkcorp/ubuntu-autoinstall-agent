<!-- file: docs/agent-tasks/core-proto/TASK-03-fleet-config.md -->
<!-- version: 1.0.0 -->
<!-- guid: 3f34af71-fecf-4cd4-952d-9c31f223eaf0 -->
<!-- last-edited: 2026-07-10 -->

# TASK-03 — FleetConfig: parameterize hardcoded fleet constants behind /etc/uaa/fleet.yaml with today's values as defaults (ws1-core)

**Priority:** P1 · **Effort:** M · **Recommended subagent:** Sonnet-class · rust-library subagent · **Why:** touches live install/verify/power paths; defaults must be provably behavior-preserving · **Depends on:** TASK-01 (wave-2 gated: CP-01 MERGED — this task FILLS the `crates/uaa-core/src/fleet.rs` stub and edits `power/mod.rs`, which CP-01 touched; skeleton collision row: serialize wave1=CP-01, wave2=CP-03)

## ⛔ START HERE (do this first, exactly)

```bash
REPO=/Users/jdfalk/repos/github.com/jdfalk/ubuntu-autoinstall-agent   # adjust to your clone
git -C "$REPO" fetch origin
git -C "$REPO" worktree add "$REPO/.worktrees/core-proto-fleet-config" -b agent/core-proto-fleet-config origin/main
cd "$REPO/.worktrees/core-proto-fleet-config"
git rebase origin/main
```

(Protocol also in `docs/agent-tasks/ORCHESTRATION.md` — the inline block above is authoritative for this task.)

## Goal

Fill the `crates/uaa-core/src/fleet.rs` stub with `FleetConfig` per spec C1 (`docs/specs/constellation-design.md`): "Fleet constants (`172.16.2.30`, `:25000`, Tang URLs, `lookup_host`, the `unimatrixone` power deny-list) move behind `FleetConfig` with current values as defaults — behavior-preserving (existing tests pass on defaults)." Provide `pub fn load_or_default() -> FleetConfig` reading `/etc/uaa/fleet.yaml` and a process-wide cached accessor, then switch the three consuming files (`crates/uaa-core/src/autoinstall/place.rs`, `crates/uaa-core/src/autoinstall/verify.rs`, `crates/uaa-core/src/power/mod.rs`) to read those values through it. The `reinstall_deny` list (default `["unimatrixone"]`) lands here for CT-06 (spec C3 one-click reinstall: "refuses `unimatrixone` (FleetConfig deny-list)") — you define the field; CT-06 consumes it. REUSE, do not invent: keep the existing `pub const` items (`DEFAULT_NETBOOT_SERVER`, `FLIP_API_PORT`, `CLOUD_INIT_BASE`, `DEFAULT_SERVER_USER`, `TARGET_AUTOINSTALL` in place.rs; `TANG_URLS`, `LUKS_PARTITION`, `LENSERV_NIC`, `CLEVIS_THRESHOLD_STR` in verify.rs; `POWER_SERVER` + the `lookup_host` registry in power/mod.rs) as the single source of the DEFAULT values — `FleetConfig::default()` references them; do NOT retype the literals in fleet.rs. Do NOT write a new YAML loader — use the `serde_yaml` dep already in the workspace.

## Background (verify before editing)

- Edge semantics (spell these into code AND tests, they are the safety property):
  - `/etc/uaa/fleet.yaml` ABSENT → silent defaults (log at debug). This is every dev/test machine today.
  - File PRESENT but unreadable/unparseable/unknown-field → **hard error, fail-closed** — a typo'd fleet.yaml silently falling back to defaults would point installs at the wrong server. Use `#[serde(deny_unknown_fields)]` (same pattern as `InstallationConfig` — verify: `grep -n "deny_unknown_fields" crates/uaa-core/src/network/ssh_installer/config.rs`).
  - Partial file → absent fields take defaults (`#[serde(default = ...)]` per field).
  - Tests must NEVER read the real `/etc/uaa/fleet.yaml`: the cached accessor honors a `UAA_FLEET_CONFIG` env override, and unit tests call `FleetConfig::default()` / `load_from(path)` directly with tempdir paths — never the global accessor.
- The power host registry stays hardcoded-as-default: `lookup_host` keeps its exact signature `pub fn lookup_host(hostname: &str) -> Option<PowerMechanism>` (CT-06 and the existing CLI call it) and becomes a thin wrapper over the fleet registry.
- `power/mod.rs` gained `pub mod dash;` / `pub mod amt_wol;` stub lines in CP-01 — leave them alone.

**HARD RULES (non-negotiable):**
- NO hardware actions. Validate ONLY in-repo (`cargo`) and, where a brief says so,
  the QEMU+swtpm harness (`scripts/vm-validate.sh`). Code that COULD touch hardware
  is written and unit-tested against mock executors only.
- NEVER wipe, write to, or deploy on 172.16.2.30 ("the server") or len-serv-003.
- `disk_device` is read from the live target at runtime, never guessed or hardcoded.
- ipmitool runs via `ssh 172.16.2.30`, never on macOS.
- NEVER power on unimatrixone (U1).
- No real secret in any file: `REPLACE_AT_PLACE_TIME` placeholders stay placeholders.
- Stay inside your worktree; never `git push`, `gh pr`, or merge — report done and stop.

**Path map:** after CP-01 (wave 1) merges, `src/**` lives at `crates/uaa-core/src/**` and the CLI at `crates/uaa/src/**`. The greps below cite pre-move paths (verifiable on today's main); at execution time run them at the old path, then the mapped path. Zero hits at BOTH = STOP and report.

- **Re-verify these anchors before editing** — line numbers drift; zero hits at both old and mapped path = STOP and report:
  ```bash
  grep -n "DEFAULT_NETBOOT_SERVER\|FLIP_API_PORT" src/autoinstall/place.rs   # expect: 2+ hits (const defs ~lines 40/46 + uses)
  grep -n "TANG_URLS\|LUKS_PARTITION" src/autoinstall/verify.rs              # expect: 2+ hits (const defs ~lines 44/50 + uses)
  grep -n "fn lookup_host" src/power/mod.rs                                  # expect: 1 hit (~line 74)
  grep -n "pub const POWER_SERVER" src/power/mod.rs                          # expect: 1 hit (~line 71)
  grep -n "//! .*fleet" crates/uaa-core/src/fleet.rs                         # expect: 1 hit (the CP-01 stub you fill; no old path exists)
  ```

## Step-by-step

1. Run the ⛔ START HERE block, then the anchor greps (mapped paths: `crates/uaa-core/src/autoinstall/place.rs`, `.../verify.rs`, `.../power/mod.rs`).
2. **Fill `crates/uaa-core/src/fleet.rs`** (keep the CP-01 header, bump version to 1.1.0):
   ```rust
   #[derive(Debug, Clone, PartialEq, serde::Deserialize)]
   #[serde(deny_unknown_fields)]
   pub struct FleetConfig {
       #[serde(default = "d_netboot_server")] pub netboot_server: String,   // = place::DEFAULT_NETBOOT_SERVER
       #[serde(default = "d_flip_api_port")]  pub flip_api_port: u16,       // = place::FLIP_API_PORT
       #[serde(default = "d_cloud_init_base")] pub cloud_init_base: String, // = place::CLOUD_INIT_BASE
       #[serde(default = "d_server_user")]    pub server_user: String,      // = place::DEFAULT_SERVER_USER
       #[serde(default = "d_tang_urls")]      pub tang_urls: Vec<String>,   // = verify::TANG_URLS
       #[serde(default = "d_luks_partition")] pub luks_partition: String,   // = verify::LUKS_PARTITION
       #[serde(default = "d_lenserv_nic")]    pub lenserv_nic: String,      // = verify::LENSERV_NIC
       #[serde(default = "d_power_server")]   pub power_server: String,     // = power::POWER_SERVER
       #[serde(default = "d_reinstall_deny")] pub reinstall_deny: Vec<String>, // = ["unimatrixone"] — consumed by CT-06
       #[serde(default = "d_power_hosts")]    pub power_hosts: Vec<PowerHostEntry>, // today's lookup_host registry
   }
   ```
   `PowerHostEntry { pub hostname: String, pub mechanism: String /* "ipmi"|"amd-dash"|"intel-amt"|"wol" */, pub bmc_host: Option<String>, pub username: Option<String> }`. Each `d_*` fn returns the EXISTING const (import it — the const stays the single source of truth). `impl Default for FleetConfig` composes the `d_*` fns. Make the existing consts `pub` where they are currently private (`TANG_URLS`, `LUKS_PARTITION`, `LENSERV_NIC` in verify.rs are `const`, not `pub const` — promote them, do not move them).
3. Add loaders to fleet.rs: `pub fn load_from(path: &Path) -> crate::error::Result<FleetConfig>` (absent file → `Ok(FleetConfig::default())`; present-but-invalid → `Err(AutoInstallError::ConfigError(...))` naming the path AND the parse error — REUSE the existing error enum, no new variants); `pub fn load_or_default() -> crate::error::Result<FleetConfig>` (path = `$UAA_FLEET_CONFIG` if set, else `/etc/uaa/fleet.yaml`); `pub fn fleet() -> &'static FleetConfig` (a `std::sync::OnceLock<FleetConfig>` — on first call runs `load_or_default()`; an invalid file at this point panics with the ConfigError text: fail-closed is REQUIRED, silent defaulting is the failure mode we are closing).
4. **Wire the three consumers** (mechanical; the observable values are IDENTICAL on defaults):
   - `place.rs`: where functions build URLs/paths from `DEFAULT_NETBOOT_SERVER`/`FLIP_API_PORT`/`CLOUD_INIT_BASE`/`DEFAULT_SERVER_USER` as runtime fallbacks, route them through `crate::fleet::fleet()` fields. Signatures that already accept an explicit `netboot_server`/server argument keep it — only the DEFAULT sourcing changes. Existing tests pass unmodified (they pass explicit values or hit the identical defaults).
   - `verify.rs`: `tang_urls`/`luks_partition`/`lenserv_nic` reads go through `fleet()`; the promoted `pub const` defaults stay for the `d_*` fns and the existing tests.
   - `power/mod.rs`: `lookup_host` re-implements over `fleet().power_hosts` (mechanism strings map to the existing `PowerMechanism` variants; unknown mechanism string → `None` + a `tracing::warn!`); `POWER_SERVER` uses stay as the default via `fleet().power_server`. The existing power tests (registry, fail-closed paths) must pass UNMODIFIED.
5. **New tests** in fleet.rs (`#[cfg(test)]`):
   - `test_defaults_match_legacy_constants` — `FleetConfig::default()` fields equal the literal legacy values (`"172.16.2.30"`, `25000`, the three Tang URLs `http://172.16.2.45/46/47`, `"/dev/nvme0n1p4"`, `"enp1s0f0"`, deny list `["unimatrixone"]`, and a `power_hosts` entry set matching today's `lookup_host` table).
   - `test_load_from_missing_file_gives_defaults` — tempdir path that doesn't exist → `Ok(default)`.
   - `test_load_from_invalid_yaml_fails_closed` — tempdir file `netboot_server: [not, a, string]` → `Err` mentioning the path; `test_load_from_unknown_field_fails_closed` — `netboot_servr: x` → `Err` (deny_unknown_fields).
   - `test_load_valid_override` — **anti-over-suppression / happy path:** a valid file overriding only `netboot_server: "10.0.0.9"` loads `Ok`, that field is overridden, every other field still equals the default (the fail-closed parser does not reject legitimate files).
   - `test_lookup_host_from_fleet_registry` — default config: `lookup_host("unimatrixone")` still returns the IPMI mechanism with `bmc_host "172.16.3.150"`, `len-serv-002` → `AmdDash`, unknown → `None`.
6. Bump the file header (version + last-edited) on every file you touch; new files get a fresh 4-line header with a new uuid4 (`uuidgen | tr 'A-F' 'a-f'`).

## How to test

```bash
cargo test --lib --offline && cargo build --offline
# Expected: all tests pass (baseline 311 + ~6 new fleet tests; ZERO pre-existing tests modified — `git diff origin/main -- '*place.rs' '*verify.rs' '*power/mod.rs' | grep "^-.*#\[test\]"` is empty), build clean
cargo clippy --offline -- -D warnings
# Expected: no warnings
grep -rn "172.16.2.30" crates/uaa-core/src/fleet.rs
# Expected: 0 hits in code (defaults reference the existing consts, literals not retyped; comments/tests may mention it)
UAA_FLEET_CONFIG=/nonexistent cargo test --lib --offline -p uaa-core fleet
# Expected: fleet tests pass with the env override pointing nowhere (defaults path exercised)
```

## Acceptance criteria

- [ ] `FleetConfig` present with all ten fields: `grep -c "netboot_server\|flip_api_port\|cloud_init_base\|server_user\|tang_urls\|luks_partition\|lenserv_nic\|power_server\|reinstall_deny\|power_hosts" crates/uaa-core/src/fleet.rs` → ≥10; `grep -n "deny_unknown_fields" crates/uaa-core/src/fleet.rs` → 1 hit.
- [ ] Fail-closed on invalid file proven: `grep -n "test_load_from_invalid_yaml_fails_closed\|test_load_from_unknown_field_fails_closed" crates/uaa-core/src/fleet.rs` → 2 hits, both green.
- [ ] Behavior-preserving: every pre-existing test passes UNMODIFIED (`git diff origin/main --stat` shows no `#[test]` fn bodies changed in place.rs/verify.rs/power/mod.rs) and `test_defaults_match_legacy_constants` passes.
- [ ] Deny-list landed: `grep -n "reinstall_deny" crates/uaa-core/src/fleet.rs` → ≥2 hits (field + default `["unimatrixone"]`).
- [ ] `lookup_host` signature unchanged and registry-backed: `grep -n "pub fn lookup_host(hostname: &str) -> Option<PowerMechanism>" crates/uaa-core/src/power/mod.rs` → 1 hit; `test_lookup_host_from_fleet_registry` green.
- [ ] **Anti-over-suppression:** `test_load_valid_override` passes — a legitimate partial fleet.yaml still loads and overrides exactly one field (the deny_unknown_fields/fail-closed guard does not block valid files).
- [ ] `cargo test --lib --offline && cargo build --offline` exits 0; clippy clean.
- [ ] File headers bumped on every changed file (`git diff origin/main --stat` files each show a version bump; guids unchanged).

## Commit message

```
feat(fleet): FleetConfig behind /etc/uaa/fleet.yaml with legacy constants as defaults (ws1-core)

Fills the CP-01 fleet.rs stub (spec C1): netboot server/port, cloud-init base,
server user, Tang URLs, LUKS partition, NIC, power server, power host registry,
and the unimatrixone reinstall deny-list (for CT-06) — every default sourced
from the existing pub consts, never retyped. load_or_default() honors
UAA_FLEET_CONFIG; missing file = silent defaults, invalid/unknown-field file =
hard ConfigError (fail-closed, deny_unknown_fields). place.rs/verify.rs/
power/mod.rs read through fleet(); lookup_host keeps its signature over the
registry. All pre-existing tests pass unmodified on defaults; 6 new fleet tests.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
```

## Done

STOP — report done with exact counts; the coordinator owns push/PR/merge.

## Idempotency / Rollback

Additive polarity: if `grep -n "pub struct FleetConfig" crates/uaa-core/src/fleet.rs` hits, the task is already applied — run the Acceptance criteria checks instead of re-applying. Rollback = revert the single commit; fleet.rs returns to the CP-01 stub and the three consumer files return to reading their consts directly — no config file is ever written by this code (it only READS `/etc/uaa/fleet.yaml`), so there is no machine state to unwind.
