<!-- file: docs/agent-tasks/installer-robustness/TASK-03-detect-network-config-parse.md -->
<!-- version: 1.0.0 -->
<!-- guid: 8d6401b0-f67f-49db-94d0-fa2d64bbfa9f -->
<!-- last-edited: 2026-07-09 -->

# TASK-03 — detect_network_config: actually parse ip -j addr / ip -j route (stop returning hardcoded eth0/dhcp) (todo:detect_network_config)

**Priority:** P2 · **Effort:** M · **Recommended subagent:** Sonnet-class · rust-backend subagent · **Why:** single-file parser rewrite; wrong values only affect the legacy no-config path. · **Depends on:** none (wave 2 — serialized after TASK-02 merges; both edit `src/cli/commands.rs`)

## ⛔ START HERE (do this first, exactly)

```bash
REPO=/Users/jdfalk/repos/github.com/jdfalk/ubuntu-autoinstall-agent   # adjust to your clone
git -C "$REPO" fetch origin
git -C "$REPO" worktree add "$REPO/.worktrees/installer-robustness-detect-network-config-parse" -b agent/installer-robustness-detect-network-config-parse origin/main
cd "$REPO/.worktrees/installer-robustness-detect-network-config-parse"
git rebase origin/main
```

(Protocol is also in `docs/agent-tasks/ORCHESTRATION.md` — the inline block above is authoritative for this task.)

## Goal

Rewrite `detect_network_config` in `src/cli/commands.rs` (the ONLY file this task touches) so it
actually parses `ip -j addr` and `ip -j route` JSON from the live target and returns a truthful
`(interface, address, gateway)` tuple: the interface owning the default route, its global IPv4
address as `CIDR` — or the literal string `"dhcp"` when the address is DHCP-assigned — and the
default gateway. Today it ignores its input and returns hardcoded `("eth0", "dhcp", "auto")`.

Reuse-don't-invent: `serde_json` is already a dependency — parse into `serde_json::Value` (no
new dependency, no new struct file). Keep `crate::error::AutoInstallError::ValidationError` for
every failure path — no new error variants. The signature change of `detect_network_config` IS
part of this task; its one and only call site is listed below.

## Background (verify before editing)

- Current behavior (scout-verified 2026-07-09): `detect_network_config(_network_info: &str)`
  discards its input and returns `("eth0", "dhcp", "auto")`. Those values feed
  `InstallationConfig.network_{interface,address,gateway}` on the legacy no-`--config` local
  path, and the netplan template renders `network_address` as a literal address — `"dhcp"` is
  not a CIDR and `"auto"` is not a gateway, so the autodetect path renders INVALID netplan on
  the installed system. Only the `--config` path is proven today.
- Contract with TASK-04 (same wave, different files): the literal string `"dhcp"` in
  `network_address` is what TASK-04's new `dhcp4: true` netplan branch keys on. Until TASK-04
  merges, a `"dhcp"` return renders as a literal address exactly like today — no regression;
  after TASK-04, it renders correctly. Do NOT edit `system_setup.rs` in this task.
- The caller chain is test-safe: `local_install_command` returns early on the
  `SystemUtils::is_root()` check under `cargo test`, so `create_local_installation_config` (and
  any `ip` subprocess you add there) is never reached by tests.
- `system_info.network_info` (free text from `ip addr show` / `ip route show` built in
  `investigation.rs`) stays DISPLAY-ONLY — do NOT edit `investigation.rs`.
- HARD RULES (restate): code-only task — never run against 172.16.2.30 ("the server") or
  len-serv-003; validation is unit tests with fixture JSON. Read live-target facts, never
  guess: if there is no default route or no global IPv4, ERROR (telling the operator to use
  `--config`) instead of returning made-up values. Workers never push/PR/merge.

**Re-verify these anchors before editing** — line numbers drift, they are a starting point only.
Zero hits = STOP and report:

```bash
grep -n 'fn detect_network_config' src/cli/commands.rs   # expect: 1 hit ~line 654; hardcoded eth0 at ~657 via grep -n 'eth0' src/cli/commands.rs (2 hits: 657 + a test at 934)
grep -n 'eth0' src/cli/commands.rs   # expect: 2 hits — ~657 (the hardcoded default THIS task removes) and ~934 (a deploy-test YAML fixture: DO NOT touch it)
grep -n 'detect_network_config(&system_info.network_info)' src/cli/commands.rs   # expect: 1 hit ~line 603 — the ONLY call site, inside create_local_installation_config
grep -n 'fn create_local_installation_config' src/cli/commands.rs   # expect: 1 hit ~line 595
grep -n 'serde_json' Cargo.toml   # expect: 1 hit — already a dependency, add nothing
```

## Step-by-step

1. Run the anchor greps. Zero hits → STOP and report. (If TASK-02 merged first, the call-site
   line numbers will have drifted — the greps still locate them.)
2. Rewrite the function as a pure parser of two JSON documents (keep the name; change the
   parameters): `fn detect_network_config(ip_addr_json: &str, ip_route_json: &str) ->
   Result<(String, String, String)>`.
   - **Route pass** (`ip -j route` output — a JSON array): find entries where
     `"dst" == "default"`; take the FIRST one (`ip` orders by metric). From it read
     `iface = entry["dev"]` and `gateway = entry["gateway"]`.
     - No default-route entry → `Err(ValidationError("no default route on the live target — cannot autodetect network; use --config"))`.
     - Default route without a `"gateway"` key → same error style (never emit `"auto"`).
   - **Addr pass** (`ip -j addr` output — a JSON array of interface objects): find the object
     with `"ifname" == iface`; scan its `"addr_info"` array for the FIRST entry with
     `"family" == "inet"` AND `"scope" == "global"`.
     - Found and the entry has `"dynamic": true` → `address = "dhcp".to_string()` (the
       DHCP-assigned case; TASK-04 consumes this exact literal).
     - Found and not dynamic → `address = format!("{}/{}", local, prefixlen)` (e.g.
       `"172.16.2.35/16"`).
     - Interface object missing, or no global inet entry →
       `Err(ValidationError("no global IPv4 address on <iface> — cannot autodetect network; use --config"))`.
   - Delete the hardcoded `eth0`/`dhcp`/`auto` literals entirely (transform, not a fallback:
     there must be NO code path that returns invented values).
3. Update the single call site in `create_local_installation_config`: run
   `std::process::Command::new("ip").args(["-j", "addr"])` and
   `std::process::Command::new("ip").args(["-j", "route"])` on the live system; on spawn error
   or non-zero exit return
   `Err(ValidationError("cannot read network state on the live target — refusing to guess (use --config)"))`;
   pass both stdouts to `detect_network_config`. Touch nothing else in that function.
4. Purely-scoped change: do not modify `detect_primary_disk` (TASK-02, already merged), the
   deploy-test YAML fixture containing `interface: eth0` (~line 934), `investigation.rs`, or
   `system_setup.rs`.
5. Add unit tests in the existing `mod tests` with inline fixture JSON (names are load-bearing):
   - `test_detect_network_config_static` — eno1 with `192.168.1.10/24` global inet (no
     `dynamic` flag) + default route via `192.168.1.1` dev eno1 →
     `("eno1", "192.168.1.10/24", "192.168.1.1")`.
   - `test_detect_network_config_dhcp` — same shape but `"dynamic": true` →
     `("eno1", "dhcp", "192.168.1.1")`.
   - `test_detect_network_config_picks_default_route_iface` — fixture with `lo` (127.0.0.1/8,
     scope host), a `docker0` with a global address but NO default route, and `eno1` holding
     the default route → returns eno1's data (anti-over-suppression: real interface still found
     through the noise).
   - `test_detect_network_config_no_default_route_errors` — route JSON `[]` → `Err`.
   - `test_detect_network_config_no_global_inet_errors` — iface has only an inet6/link-local
     entry → `Err`.
6. Bump the `src/cli/commands.rs` file header (`version` + `last-edited`, keep `guid`). Run the
   gate, commit.

## How to test

```bash
cargo test --lib --offline    # Expected: 237+ passed; 0 failed (baseline 237 + the new detect tests)
cargo build --offline         # Expected: exit 0
cargo clippy --offline        # Expected: no new warnings
```

## Acceptance criteria

- [ ] `grep -n '"eth0".to_string()' src/cli/commands.rs` → 0 hits (hardcoded tuple deleted);
  `grep -n '"auto".to_string()' src/cli/commands.rs` → 0 hits.
- [ ] `grep -n 'eth0' src/cli/commands.rs` → exactly 1 hit, the deploy-test YAML fixture
  (~934) — untouched.
- [ ] `grep -n '"dst"\|addr_info' src/cli/commands.rs` → ≥2 hits (both parsers present).
- [ ] `grep -c 'fn test_detect_network_config' src/cli/commands.rs` → 5.
- [ ] Edge semantics enforced: `cargo test --lib --offline test_detect_network_config_no_default_route_errors`
  and `... test_detect_network_config_no_global_inet_errors` pass (failure paths ERROR, never
  invent values).
- [ ] Anti-over-suppression: `cargo test --lib --offline test_detect_network_config_picks_default_route_iface`
  passes — the real interface is still detected with lo/docker0 noise present.
- [ ] DHCP contract for TASK-04: `cargo test --lib --offline test_detect_network_config_dhcp`
  passes and returns the exact literal `"dhcp"`.
- [ ] Tests green: `cargo test --lib --offline` 237+ passed, 0 failed; `cargo build --offline`
  and `cargo clippy --offline` clean.
- [ ] File headers bumped (`grep -n 'last-edited:' src/cli/commands.rs` shows today's date; guid
  unchanged).

## Commit message

```
fix(cli): detect_network_config parses ip -j addr/route instead of returning hardcoded eth0/dhcp

The legacy no-config local path fed literal ("eth0", "dhcp", "auto") into the
netplan template — an invalid rendering ("dhcp" is not a CIDR, "auto" is not a
gateway). Now parses ip -j route for the default-route interface and gateway and
ip -j addr for that interface's global IPv4, returning CIDR for static
addresses and the literal "dhcp" for DHCP-assigned ones (consumed by the
netplan dhcp4 branch). Missing default route or global IPv4 is a hard error
pointing at --config — values are never invented.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
```

## Done

STOP — report done with exact counts; the coordinator owns push/PR/merge.

## Idempotency / Rollback

Already-done check (transform polarity — new parsers present AND hardcoded tuple absent): if
`grep -n 'addr_info' src/cli/commands.rs` hits AND
`grep -n '"eth0".to_string()' src/cli/commands.rs` returns 0 hits, the task is already
applied — run the acceptance checks instead of re-applying. Rollback: `git revert` the single
commit — restores the hardcoded tuple and the old call site; no data or on-disk state is
touched, siblings unaffected.
