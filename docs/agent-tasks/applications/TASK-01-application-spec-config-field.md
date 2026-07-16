<!-- file: docs/agent-tasks/applications/TASK-01-application-spec-config-field.md -->
<!-- version: 1.0.0 -->
<!-- guid: 304bef0b-84ea-4fc7-8ce7-08d573007cf9 -->
<!-- last-edited: 2026-07-16 -->

# TASK-01 — Add `ApplicationSpec` + the defaulted `applications` field to `InstallationConfig` (DS-APP-01)

**Priority:** P1 · **Effort:** M · **Recommended subagent:** Sonnet-class · rust-core subagent · **Why:** touches `InstallationConfig`'s serde contract — a missing `#[serde(default)]` silently breaks all five committed host YAMLs, and four exhaustive struct literals fail to compile without the new field. · **Depends on:** none (wave 1)

## ⛔ START HERE (do this first, exactly)

```bash
REPO=/Users/jdfalk/repos/github.com/jdfalk/ubuntu-autoinstall-agent   # adjust to your clone
git -C "$REPO" fetch origin
git -C "$REPO" worktree add "$REPO/.worktrees/applications-application-spec-config-field" -b agent/applications-application-spec-config-field origin/main
cd "$REPO/.worktrees/applications-application-spec-config-field"
git rebase origin/main
```

(Protocol also in `docs/agent-tasks/ORCHESTRATION.md` — the inline block above is authoritative for this task.)

## Goal

Add the `ApplicationSpec` enum + `CockroachSpec` struct to `crates/uaa-core/src/network/ssh_installer/config.rs`, and add one new field to `InstallationConfig`:

```rust
/// Applications to install into the target during Phase 5. Empty = none,
/// which is exactly today's behavior for every committed host config.
#[serde(default)]
pub applications: Vec<ApplicationSpec>,
```

This is **purely additive and behavior-neutral**: an empty list is today's behavior, and every committed YAML omits the key so `#[serde(default)]` supplies `vec![]`. Nothing consumes the field yet — Phase-5 wiring is TASK-02, the Cockroach install step is TASK-03. Do NOT wire anything into the installer in this task.

REUSE — do not invent parallels:

- **The serde-default convention already in this file.** Verify: `grep -n "fn default_tang_threshold\|fn default_true\|fn default_tpm2_pcr_ids\|pub fn default_install_ca_cert\|pub fn default_network_renderer" crates/uaa-core/src/network/ssh_installer/config.rs` (5 hits). Every optional field pairs `#[serde(default = "fn_name")]` with a `fn default_x() -> T`. Follow that shape exactly for `CockroachSpec`'s defaulted fields. Do NOT add a `Default` impl to `InstallationConfig` — it deliberately has none.
- **`crate::error::AutoInstallError`** for any error path. Do NOT introduce a new error enum.
- Do NOT add any dependency to `Cargo.toml`. `serde`/`serde_yaml` are already present.

## Background (verify before editing)

- `InstallationConfig` carries `#[serde(deny_unknown_fields)]`. A field **without** a serde default makes every committed YAML fail to parse, because none of them contain an `applications:` key. The default is not optional — it is what makes this task behavior-neutral.
- `InstallationConfig` has **no `Default` impl** and **no literal uses `..Default::default()`** (verified: 0 hits). Every construction site is an exhaustive struct literal, so adding a field is a **compile error at 4 sites** until each gets `applications: vec![]` (or `Vec::new()`). Three are test helpers; **one is a live CLI path** — do not miss it.
- The four literal sites to fix (re-verify with the grep block below — never trust these paths blind):
  1. `crates/uaa/src/cli/commands.rs` — `Ok(InstallationConfig {` — **live CLI code**, not a test.
  2. `crates/uaa-core/src/network/ssh_installer/config.rs` — inside `for_len_serv_003()`.
  3. `crates/uaa-core/src/network/ssh_installer/system_setup.rs` — inside `sample_netplan_config`.
  4. `crates/uaa-core/src/network/ssh_installer/installer.rs` — inside `sample_config`.
- **`ApplicationSpec` lives in `config.rs`, not in a new module.** It is part of the installer's contract (a field of `InstallationConfig`), and `crates/uaa-core/src/profile/` (a later task) will import it *from here*. Putting it elsewhere inverts the dependency and blocks profiles/TASK-01.
- The `CockroachSpec` field values below are **verified live fleet facts** read from `172.16.2.30:/var/www/html/cloud-init/scripts/len-serv-003-variables.sh` on 2026-07-16. They are defaults/documentation only — this task installs nothing.
- Edge semantics (spelled out here AND in acceptance):
  - **`applications:` key absent from YAML** → `vec![]`, NOT an error. This is every committed config today and must keep parsing byte-identically.
  - **`applications: []` explicitly** → also `vec![]`; indistinguishable from absent. Both are valid.
  - **Unknown `kind:` value** (e.g. `kind: redis`) → **hard parse error naming the unknown kind**. The enum is closed by design (spec Decision 15); an unknown application must never be silently dropped, because silently dropping it would deploy a machine missing its workload.
  - **`deny_unknown_fields` applies to the nested specs too** → a typo'd key inside a `cockroach:` block is a hard error, same as at the top level.

**HARD RULES (non-negotiable):**
- NO hardware actions. Validate ONLY in-repo (`cargo`). This task touches no executor and runs no command.
- NEVER wipe, write to, or deploy on 172.16.2.30 ("the server") or len-serv-003.
- NEVER power on unimatrixone (U1).
- No real secret in any file: `REPLACE_AT_PLACE_TIME` placeholders stay placeholders.
- Do NOT edit any file under `examples/configs/install/` — the whole point of the serde default is that those files need no change. If you find yourself editing one, you have the wrong design.
- Stay inside your worktree; never `git push`, `gh pr`, or merge — report done and stop.

- **Re-verify these anchors before editing** — line numbers drift; zero hits = STOP and report:
  ```bash
  grep -n "pub struct InstallationConfig" crates/uaa-core/src/network/ssh_installer/config.rs
  # expect: 1 hit (~line 43) — the struct to extend
  grep -n '#\[serde(deny_unknown_fields)\]' crates/uaa-core/src/network/ssh_installer/config.rs
  # expect: 1 hit (~line 42) — the attribute that makes the serde default mandatory
  grep -n "pub fn for_len_serv_003" crates/uaa-core/src/network/ssh_installer/config.rs
  # expect: 1 hit (~line 138) — literal site 2
  grep -rn "InstallationConfig {" --include="*.rs" crates/
  # expect: exactly 7 hits — 4 are literals to fix (commands.rs, config.rs impl,
  #         system_setup.rs sample_netplan_config, installer.rs sample_config);
  #         the other 3 are the struct def + impl block + a fn return type.
  grep -n "fn test_install_example_configs_round_trip" crates/uaa-core/src/network/ssh_installer/config.rs
  # expect: 1 hit (~line 252) — the test pinning the four committed YAMLs; must stay green UNCHANGED
  grep -n "fn test_unknown_yaml_key_rejected" crates/uaa-core/src/network/ssh_installer/config.rs
  # expect: 1 hit (~line 328) — pins deny_unknown_fields; must stay green
  ```

## Step-by-step

1. Open `crates/uaa-core/src/network/ssh_installer/config.rs` (use the greps above — never trust line numbers from this brief).
2. Add the two new types near the top, after `TangServer` and before `InstallationConfig`:

   ```rust
   /// A workload assignable to a host. Closed-but-growing by design (spec
   /// Decision 15): adding HAProxy/Keepalived later is a new variant, not a
   /// plugin framework. An unknown `kind` is a hard parse error — never a
   /// silent skip, because a silently-dropped application deploys a machine
   /// missing its workload.
   #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
   #[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
   pub enum ApplicationSpec {
       Cockroach(CockroachSpec),
   }

   /// CockroachDB node parameters. `advertise`/`join` are NOT here: they are
   /// DERIVED per host from the group's sibling list (profiles/TASK-04), never
   /// authored. Defaults are the live fleet's values (verified 2026-07-16).
   #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
   #[serde(deny_unknown_fields)]
   pub struct CockroachSpec {
       #[serde(default = "default_cockroach_version")]
       pub version: String,
       #[serde(default = "default_cockroach_port")]
       pub port: u16,
       #[serde(default = "default_cockroach_sql_port")]
       pub sql_port: u16,
       #[serde(default = "default_cockroach_http_addr")]
       pub http_addr: String,
       /// Cluster seed, always first in the join string.
       pub seed_ip: String,
       #[serde(default = "default_cockroach_cache")]
       pub cache: String,
       #[serde(default = "default_cockroach_max_sql")]
       pub max_sql_memory: String,
       #[serde(default = "default_cockroach_locality")]
       pub locality: String,
   }
   ```

3. Add the paired default fns next to the existing `default_*` fns (mirror their style exactly):
   `default_cockroach_version() -> String` = `"v25.3.0"`, `default_cockroach_port() -> u16` = `36357`,
   `default_cockroach_sql_port() -> u16` = `36257`, `default_cockroach_http_addr() -> String` = `":38080"`,
   `default_cockroach_cache() -> String` = `".25"`, `default_cockroach_max_sql() -> String` = `".25"`,
   `default_cockroach_locality() -> String` = `"region=us,cluster-unit=lenovo"`.
4. Add the field to `InstallationConfig`, last, after `install_ca_cert`:
   ```rust
   #[serde(default)]
   pub applications: Vec<ApplicationSpec>,
   ```
5. Fix all four struct literals by adding `applications: Vec::new(),` (or `vec![]`). Use the `grep -rn "InstallationConfig {"` output — do not guess. **`crates/uaa/src/cli/commands.rs` is live code, not a test; it must compile too.**
6. Keep the change purely additive — do not reorder existing fields, do not touch any existing `default_*` fn, do not change any signature, do not edit any file under `examples/configs/install/`.
7. Add tests to `config.rs`'s existing `mod tests` (mirror the style of `test_multikey_serde_defaults_when_absent`):
   - `test_applications_defaults_to_empty_when_absent` — a minimal YAML with no `applications:` key parses, `cfg.applications.is_empty()`.
   - `test_applications_empty_is_todays_behavior` — `InstallationConfig::for_len_serv_003().applications.is_empty()`.
   - `test_cockroach_spec_defaults` — a `cockroach` spec with only `seed_ip` set yields version `v25.3.0`, port `36357`, sql_port `36257`, cache `.25`, locality `region=us,cluster-unit=lenovo`.
   - `test_unknown_application_kind_rejected` — `kind: redis` fails to parse **and the error names `redis`** (assert on the error string, mirroring `test_unknown_yaml_key_rejected`'s `err.to_string().contains(...)` shape).
   - `test_cockroach_spec_unknown_field_rejected` — a typo'd key inside the cockroach block is a parse error.
8. Bump the file header (`version` + `last-edited`) on every file you touch; keep existing guids.

**Anti-over-suppression: N/A** — this task adds no filter, guard, veto, skip, or dedupe path. It adds a data field with a default; nothing is conditionally excluded. (`test_applications_empty_is_todays_behavior` and the unchanged `test_install_example_configs_round_trip` together prove the existing happy path is untouched.)

## How to test

```bash
cargo test --lib --offline
# Expected: 634+ passed, 0 failed (baseline is 634: 50 uaa-control + 191 uaa
#           + 391 uaa-core + 2 uaa-proto), including your 5 new tests.
cargo build --offline
# Expected: exit 0, no errors.
cargo clippy --offline -- -D warnings
# Expected: no warnings.
```

## Acceptance criteria

- [ ] `cargo test --lib --offline` exits 0 with ≥639 passed (634 baseline + 5 new) — verify: `cargo test --lib --offline 2>&1 | grep -E "^test result"`
- [ ] `cargo build --offline` exits 0 — verify: `cargo build --offline && echo BUILD_OK`
- [ ] `grep -c "applications" crates/uaa-core/src/network/ssh_installer/config.rs` returns ≥3 (field + default attr + tests)
- [ ] `grep -rn "InstallationConfig {" --include="*.rs" crates/ | wc -l` still returns 7 — no literal was added or deleted, only fixed
- [ ] `git diff --name-only origin/main -- examples/` returns **empty** — no committed YAML was edited (the serde default is what makes this unnecessary)
- [ ] `test_install_example_configs_round_trip` passes **without modification** — verify: `git diff origin/main -- crates/uaa-core/src/network/ssh_installer/config.rs | grep -c "^-.*test_install_example_configs_round_trip"` returns 0
- [ ] Unknown kind is rejected and names the kind — verify: `cargo test --lib --offline test_unknown_application_kind_rejected`
- [ ] Anti-over-suppression: N/A (stated above; task adds no filter/guard/veto/skip/dedupe path)
- [ ] `cargo clippy --offline -- -D warnings` clean
- [ ] File headers bumped (`version` + `last-edited: 2026-07-16` or later) on every changed file — verify: `git diff origin/main --name-only | xargs -I{} grep -l "last-edited: 2026-07" {}`

## Commit message

```
feat(config): add ApplicationSpec and defaulted applications field (DS-APP-01)

Adds the closed ApplicationSpec enum (Cockroach variant) and CockroachSpec
to InstallationConfig, plus a #[serde(default)] applications field. Empty is
today's behavior for every committed host config, so all five example YAMLs
parse unchanged and test_install_example_configs_round_trip stays green
without modification.

The serde default is load-bearing: InstallationConfig carries
deny_unknown_fields and has no Default impl, so the field is both mandatory
at every one of the four exhaustive struct literals and absent from every
committed YAML. An unknown application kind is a hard parse error rather
than a silent skip — a dropped application would deploy a machine missing
its workload.

Nothing consumes the field yet; Phase-5 wiring is DS-APP-02.

Co-Authored-By: Claude <noreply@anthropic.com>
```

## Done

STOP — report done with exact counts; the coordinator owns push/PR/merge.

## Idempotency / Rollback

**Polarity: additive.** If `grep -n "pub enum ApplicationSpec" crates/uaa-core/src/network/ssh_installer/config.rs` hits, this task is already applied — run the acceptance checks instead of re-applying. Rollback = revert the single commit; no data, schema, or committed YAML is touched, and `applications: []` being today's behavior means nothing downstream regresses. Siblings are unaffected except those that also edit `config.rs`, `installer.rs`, `system_setup.rs`, or `crates/uaa/src/cli/commands.rs` — see the collision table in `../BREAKDOWN-2026-07-16.md`.
