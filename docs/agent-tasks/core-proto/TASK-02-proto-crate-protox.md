<!-- file: docs/agent-tasks/core-proto/TASK-02-proto-crate-protox.md -->
<!-- version: 1.0.0 -->
<!-- guid: f990ce8f-928e-4ab2-a9bd-677be05251c0 -->
<!-- last-edited: 2026-07-10 -->

# TASK-02 — uaa-proto crate: proto/uaa/** packages, protox build.rs, full workspace.dependencies population (ws1-core)

**Priority:** P1 · **Effort:** M · **Recommended subagent:** Sonnet-class · rust-build-plumbing subagent · **Why:** new crate + build plumbing; schema mirrors the spec proto surface verbatim · **Depends on:** TASK-01 (wave-2 gated: CP-01 MERGED to origin/main and this worktree rebased — TASK-01 and this task collide on the root `Cargo.toml`, skeleton collision row: serialize wave1=CP-01, wave2=CP-02)

## ⛔ START HERE (do this first, exactly)

```bash
REPO=/Users/jdfalk/repos/github.com/jdfalk/ubuntu-autoinstall-agent   # adjust to your clone
git -C "$REPO" fetch origin
git -C "$REPO" worktree add "$REPO/.worktrees/core-proto-proto-crate-protox" -b agent/core-proto-proto-crate-protox origin/main
cd "$REPO/.worktrees/core-proto-proto-crate-protox"
git rebase origin/main
```

(Protocol also in `docs/agent-tasks/ORCHESTRATION.md` — the inline block above is authoritative for this task.)

## Goal

Purely additive: create `proto/uaa/{control,enroll,web,pxe,update}/v1/*.proto` mirroring the spec's "Proto surface" section verbatim (spec Decisions 2 and 18, `docs/specs/constellation-design.md`), a new `crates/uaa-proto` crate whose `build.rs` compiles them with pure-Rust **protox** (Decision 2: no protoc C dependency, no network, musl-clean), and — in ONE pass — populate the root `[workspace.dependencies]` table with EVERY dep the later constellation crates need (tonic, prost, protox, axum, utoipa, tower-http, rustls, tokio-rustls, rcgen, x509-parser, mdns-sd, ed25519-dalek, rust-embed, tokio-postgres, semver, notify) so no later task ever edits the root manifest again (skeleton shared_state). Generated code is NOT committed (build.rs output only). Conventions per Decision 18 + org gcommon house style: proto3, `*Request`/`*Response` wrappers, no field reuse, versioned packages, breaking change = new vN package. Do NOT add any server/runtime code — this task is schema + build plumbing only.

## Background (verify before editing)

- No tonic/prost/proto exists anywhere today (grep below). uaa-proto is the FIRST consumer of the new deps.
- The root manifest is a virtual workspace after CP-01 (`members = ["crates/*"]` glob — you do NOT touch the members list; the new crate is picked up automatically).
- Timestamps in v1 messages are RFC3339 **strings** (keep the surface simple; no well-known-type imports needed).
- Version compatibility is load-bearing: tonic, prost, and protox each pin a prost ecosystem version — pick a mutually compatible trio (protox's README states which prost it targets; tonic's changelog states its prost). A mismatch fails at build.rs time, not at runtime — that failure is loud and acceptable to iterate on.
- **Network exception (this task only):** new deps must enter `Cargo.lock`, so ONE online `cargo fetch` (or the implicit fetch of the first `cargo build`) is allowed. Every gate afterwards runs `--offline`. This is dependency vendoring, not a hardware action.

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
  grep -rn "tonic\|prost" Cargo.toml src/ | grep -v Binary    # expect: 0 hits (at execution time: grep Cargo.toml crates/ instead of src/ — still 0)
  grep -n 'members = \["crates/\*"\]' Cargo.toml              # expect: 1 hit (CP-01 merged; if 0 hits you are running before wave 1 — STOP)
  grep -n "\[workspace.dependencies\]" Cargo.toml             # expect: 1 hit (the table CP-01 created; you extend it)
  ```

## Step-by-step

1. Run the ⛔ START HERE block, then the anchor greps. Zero hits on the members-glob grep means CP-01 is not merged — STOP and report.
2. **Extend `[workspace.dependencies]`** in the root `Cargo.toml` — append (one pass, alphabetical within the new block, comment `# constellation deps (CP-02) — later crates reference these with workspace = true`): `tonic`, `tonic-build`, `prost`, `prost-types`, `protox`, `axum`, `utoipa`, `tower-http` (features `["fs"]`), `rustls`, `tokio-rustls`, `rcgen`, `x509-parser`, `mdns-sd`, `ed25519-dalek`, `rust-embed`, `tokio-postgres`, `semver`, `notify`. Choose current stable versions; the tonic/prost/protox trio MUST be mutually compatible (see Background). Run `cargo fetch` once (the network exception) so everything lands in `Cargo.lock`.
3. **Write the five proto files** (each gets a `//` 4-line header comment — `// file:`, `// version: 1.0.0`, `// guid:` fresh uuid, `// last-edited: 2026-07-10` — proto accepts `//` comments before `syntax`). RPC surface is normative from the spec; message fields below are the v1 contract (all timestamps RFC3339 strings; every RPC has dedicated Request/Response types except the shared `Ack`):
   - `proto/uaa/control/v1/control.proto` — `package uaa.control.v1;` service `ControlService`: `ApproveMachine(ApproveMachineRequest) returns (ApproveMachineResponse)` (req: `mac`, `approved_by`; resp: `saga_id`, `ok`, `detail`); `GetMachine(GetMachineRequest) returns (Machine)` (req: `mac`); `ListMachines(ListMachinesRequest) returns (ListMachinesResponse)` (resp: `repeated Machine machines`); `RecordInstallEvent(RecordInstallEventRequest) returns (Ack)` (req: `event_id`, `mac`, `status`, `detail_json`, `started_at`, `finished_at` — `event_id` is the client-minted WAL-dedup UUID, spec Decision 4); `ReinstallMachine(ReinstallMachineRequest) returns (ReinstallMachineResponse)` (req: `mac`, `confirm_cooldown_override`; resp: `saga_id`, `ok`, `detail`); `UpsertDiscoveredMac(UpsertDiscoveredMacRequest) returns (Ack)` (req: `mac`, `arch_hint`, `vendor_class`, `seen_at`). Message `Machine` mirrors the CRDB `machines` table columns (spec Data model) — `mac`, `hostname`, `ip`, `type`, `status`, `boot_target`, `tpm_ek`, `registered_at`, `approved_at`, `last_seen`, `last_ip`, `installed_at`, `last_install_status`, `updated_at` — PLUS the Decision-13 reconciliation fields `web_layer_target`, `pxe_layer_target`, `consistent` (bool). Message `Ack`: `ok` (bool), `detail`.
   - `proto/uaa/enroll/v1/enroll.proto` — service `EnrollService`: `SubmitCsr(SubmitCsrRequest) returns (SubmitCsrResponse)` (req: `csr_pem`, `claimed_hostname`, `claimed_mac`; resp: `spki_fingerprint`, `state`); `GetCredential(GetCredentialRequest) returns (GetCredentialResponse)` (req: `spki_fingerprint`; resp: `state` — one of `pending|approved|issued|rejected|revoked|superseded` per spec C6 — `cert_pem`, `ca_pem`).
   - `proto/uaa/web/v1/web.proto` — service `WebService`: `PlaceSeed(PlaceSeedRequest) returns (Ack)` (req: `mac`, `user_data`, `meta_data`, `vendor_data`, `network_config`, `uaa_config` — non-secret fields + `REPLACE_AT_PLACE_TIME` placeholders ONLY, gate enforced by WB-02); `PlaceIpxe(PlaceIpxeRequest) returns (Ack)` (req: `hostname`, `content`); `FlipBootTarget(FlipBootTargetRequest) returns (FlipBootTargetResponse)` (req: `hostname`, `target`; resp: `ok`, `detail` — missing iPXE file is `ok:false`, not an error); `RemoveHost(RemoveHostRequest) returns (Ack)` (req: `hostname`, `mac`); `ListIsos(ListIsosRequest) returns (ListIsosResponse)` (resp: `repeated IsoInfo` — `name`, `size_bytes`, `sha256`); `BuildIso(BuildIsoRequest) returns (BuildIsoResponse)` (req: `base_iso`, `profile`; resp: `job_id` — detached job, spec C4); `GetBuildJob(GetBuildJobRequest) returns (BuildJob)` (req: `job_id`; `BuildJob`: `job_id`, `state`, `detail`, `artifact_path`); `PublishAgentBinary(PublishAgentBinaryRequest) returns (Ack)` (req: `name`, `version`, `target`, `artifact` bytes, `signature` bytes — sig verified before placement).
   - `proto/uaa/pxe/v1/pxe.proto` — service `PxeService`: `SetupPxe(SetupPxeRequest) returns (Ack)` (req: `mac`, `hostname`, `boot_target`); `SetBootTarget(SetBootTargetRequest) returns (Ack)` (req: `mac`, `boot_target`); `Health(HealthRequest) returns (HealthResponse)` (resp: `dnsmasq_active`, `tftpd_active`, `tftp_probe_ok` bools + `repeated BootTargetState targets` — `mac`, `expected`, `applied`, `consistent`); `StreamDiscoveredMacs(StreamDiscoveredMacsRequest) returns (stream DiscoveredMac)` (`DiscoveredMac`: `mac`, `first_seen`, `last_seen`, `arch_hint`, `vendor_class`, `dismissed`); `SetDnsRecord(SetDnsRecordRequest) returns (Ack)` (req: `hostname`, `ip`, `remove` bool).
   - `proto/uaa/update/v1/update.proto` — messages only, no service (served as JSON by uaa-web, spec C7): `Manifest` (`repeated BinaryEntry binaries`, `min_version`), `BinaryEntry` (`name`, `version`, `target`, `sha256`, `sig`, `url`).
4. **Create `crates/uaa-proto`**: `Cargo.toml` (`name = "uaa-proto"`, workspace package fields; `[dependencies] tonic = { workspace = true }`, `prost = { workspace = true }`, `prost-types = { workspace = true }`; `[build-dependencies] protox = { workspace = true }`, `tonic-build = { workspace = true }`); `build.rs` (fresh header):
   ```rust
   fn main() -> Result<(), Box<dyn std::error::Error>> {
       let files = [
           "../../proto/uaa/control/v1/control.proto",
           "../../proto/uaa/enroll/v1/enroll.proto",
           "../../proto/uaa/web/v1/web.proto",
           "../../proto/uaa/pxe/v1/pxe.proto",
           "../../proto/uaa/update/v1/update.proto",
       ];
       let fds = protox::compile(files, ["../../proto"])?;
       tonic_build::configure().build_server(true).build_client(true).compile_fds(fds)?;
       println!("cargo:rerun-if-changed=../../proto");
       Ok(())
   }
   ```
   (build.rs runs with cwd = the crate dir, hence `../../proto`. If the pinned tonic-build names its FileDescriptorSet entry point differently — e.g. `compile_fds_with_config` — adapt the one call; the protox-not-protoc requirement is non-negotiable.)
5. `crates/uaa-proto/src/lib.rs` (fresh header): one nested module per package re-exporting the generated code, e.g. `pub mod control { pub mod v1 { tonic::include_proto!("uaa.control.v1"); } }` — repeat for `enroll`, `web`, `pxe`, `update`.
6. **Smoke tests** (`#[cfg(test)]` in lib.rs, so they join the `--lib` count): `test_machine_roundtrip` — build a `control::v1::Machine` with `mac`, `boot_target`, `consistent: true`, prost-encode then decode, assert equality; `test_manifest_min_version_field` — build an `update::v1::Manifest` with one `BinaryEntry` and `min_version: "1.2.3"`, encode/decode, assert `min_version` survives.
7. Verify no generated code is committed: generated files live under `target/` via `OUT_DIR` (the default) — `git status` must show only the files this brief names.
8. Bump the file header (version + last-edited) on every file you touch; new files get a fresh 4-line header with a new uuid4 (`uuidgen | tr 'A-F' 'a-f'`).

## How to test

```bash
cargo fetch
# Expected: exit 0 (the ONE allowed network op; populates Cargo.lock)
cargo test --lib --offline && cargo build --offline
# Expected: all tests pass (baseline 311 + 2 new uaa-proto smoke tests = 313), build clean
cargo clippy --offline -- -D warnings
# Expected: no warnings
rustup target add x86_64-unknown-linux-musl && cargo check --offline --target x86_64-unknown-linux-musl --workspace
# Expected: exit 0 (musl gate — `check` needs no musl C toolchain; CI musl-build.yml does the full static link)
grep -c "^service " proto/uaa/*/v1/*.proto
# Expected: control=1, enroll=1, web=1, pxe=1, update=0
git status --porcelain | grep -v "proto/\|crates/uaa-proto/\|Cargo.toml\|Cargo.lock"
# Expected: empty (no generated code, nothing else touched)
```

## Acceptance criteria

- [ ] Five proto files exist with versioned packages: `grep -l "^package uaa\." proto/uaa/*/v1/*.proto | wc -l` → 5; all RPC names from the spec surface present: `grep -c "rpc " proto/uaa/control/v1/control.proto` → 6, `enroll` → 2, `web` → 8, `pxe` → 5.
- [ ] protox, not protoc: `grep -n "protox::compile" crates/uaa-proto/build.rs` → 1 hit; `grep -rn "protoc" crates/uaa-proto/` → 0 hits.
- [ ] Workspace deps populated in one pass: `grep -c "tonic\|prost\|protox\|axum\|utoipa\|tower-http\|rustls\|rcgen\|x509-parser\|mdns-sd\|ed25519-dalek\|rust-embed\|tokio-postgres\|semver\|notify" Cargo.toml` → ≥16 (every named dep present in `[workspace.dependencies]`).
- [ ] Members glob untouched: `grep -c 'members = \["crates/\*"\]' Cargo.toml` → 1 and no explicit member list added.
- [ ] Smoke tests pass: `grep -n "test_machine_roundtrip\|test_manifest_min_version_field" crates/uaa-proto/src/lib.rs` → 2 hits, both green in the suite.
- [ ] Anti-over-suppression: N/A
- [ ] `cargo test --lib --offline && cargo build --offline` exits 0; clippy clean; `cargo check --offline --target x86_64-unknown-linux-musl --workspace` exits 0.
- [ ] File headers bumped on every changed file (`git diff origin/main --stat` files each show a version bump; guids unchanged; new files carry fresh guids).

## Commit message

```
feat(proto): add uaa-proto crate (protox build of proto/uaa/**) + full workspace deps (ws1-core)

proto/uaa/{control,enroll,web,pxe,update}/v1 packages mirror the constellation
spec proto surface (Decisions 2/18): 21 RPCs across 4 services + update manifest
types, proto3, Request/Response wrappers, RFC3339-string timestamps. build.rs
compiles with pure-Rust protox (no protoc, no network); generated code stays in
OUT_DIR. Root [workspace.dependencies] gains every anticipated constellation dep
in one pass (tonic/prost/protox/axum/utoipa/tower-http/rustls/tokio-rustls/
rcgen/x509-parser/mdns-sd/ed25519-dalek/rust-embed/tokio-postgres/semver/notify)
so later crates only reference workspace = true. 2 encode/decode smoke tests.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
```

## Done

STOP — report done with exact counts; the coordinator owns push/PR/merge.

## Idempotency / Rollback

Additive polarity: if `grep -n "protox::compile" crates/uaa-proto/build.rs` hits and `test -d proto/uaa/control/v1` succeeds, the task is already applied — run the Acceptance criteria checks instead of re-applying. Rollback = revert the single commit; it removes `proto/**`, `crates/uaa-proto/**`, and the appended `[workspace.dependencies]` entries — `crates/uaa-core`, `crates/uaa`, and all 311 baseline tests stay untouched (nothing depends on uaa-proto yet).
