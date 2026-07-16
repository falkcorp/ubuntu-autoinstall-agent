<!-- file: docs/specs/deploy-system-design.md -->
<!-- version: 2.1.0 -->
<!-- guid: 26edfc98-f387-4a4a-9c42-f627dc20820b -->
<!-- last-edited: 2026-07-16 -->

# Deployment System — Profiles, Applications, Check-in — Design Spec

**Status:** Draft — Gate 2 <!-- flip to: Approved — ready for implementation planning -->
**Scope:** `crates/uaa-core` (profile schema, merge/resolve, Phase-5 application install), `crates/uaa-control` (profile store, index allocation, drift review, application check-in), `scripts/vm-validate.sh` (gate), `web/` (operator SPA). Explicit follow-ups in Non-goals.
**Revision:** v2.0.0 — rewritten after the design-judge panel (correctness / ops-rollback / simplicity-scope). The panel overturned the v1 persistence architecture; see Decision 4.

---

## Motivation

Every machine this repo installs is configured by a hand-authored flat YAML under `examples/configs/install/`. There are five, and they are near-duplicates. Diffing them (verified 2026-07-16) shows the real per-host variance is tiny:

| Field | len-serv-001 | len-serv-002 | len-serv-003 | unimatrixone | vm-test |
|---|---|---|---|---|---|
| `hostname` | len-serv-001 | len-serv-002 | len-serv-003 | unimatrixone | vm-test |
| `network_address` | 172.16.3.92/23 | 172.16.3.94/23 | 172.16.3.96/23 | 172.16.2.35/23 | 10.0.2.15/24 |
| `disk_device` | /dev/nvme0n1 | /dev/nvme0n1 | /dev/nvme0n1 | **/dev/md/Volume0_0** | /dev/vda |
| `network_interface` | enp1s0f0 | enp1s0f0 | enp1s0f0 | **ens5f0** | enp0s3 |
| `network_gateway` | 172.16.2.1 | 172.16.2.1 | 172.16.2.1 | 172.16.2.1 | 10.0.2.2 |
| `network_search` | jf.local | jf.local | jf.local | jf.local | vm.local |
| `tang_servers` | 3 | 3 | 3 | 3 | **[]** |
| `expect_fido2` | true | true | true | true | **false** |
| `timezone`, `debootstrap_release`, `debootstrap_mirror`, `initramfs_type`, `enroll_tpm2`, `tpm2_pcr_ids` | identical across all five | | | | |

Across the three Lenovo hosts, **only `hostname` and `network_address` differ at all.**

Three gaps follow:

1. **No concept of a role.** `InstallationConfig` is flat and per-host. `config_place.rs` encodes the fleet as a hardcoded `KNOWN_HOSTS: [&str; 4]` plus a `mac_for_host()` match with one arm per machine — a compile-time host registry.
2. **The native installer cannot stand up a CockroachDB node.** `cockroach_advertise`/`cockroach_join` live only in the retiring curtin path's `HostSpec` as template placeholders. The actual install is `setup_cockroachdb.sh`, which exists **only on the netboot server, outside git**, fetched at first boot over plain HTTP and `rm -- "$0"`-ed after running.
3. **"Status of systems checking in" covers registration only.** `MachineStatus` is `Seen|Pending|Approved|Revoked|Unknown`; `last_install_status` is a free-form string written once at the final webhook. Nothing tracks whether an installed machine's applications are healthy.

**Goal:** one declarative model — group defaults + per-host overrides + assignable applications — resolved into the existing flat `InstallationConfig`, so any machine class deploys and verifies without a hand-authored per-host YAML.

## Goals

- Express a machine class once (`HostGroupProfile`); express a machine as only its irreducible facts (`HostProfile`).
- Assign workloads (`Application`) at either level; a host's effective set is the union.
- Allocate hostname indices **once**, bound to a durable identity, never re-derived — surviving group deletion.
- Detect out-of-band edits to profile objects and require an explicit accept-or-revert.
- Report post-install application health, and distinguish "reported healthy" from "hasn't reported".
- Stand a CockroachDB node up from the native installer, **provably** in QEMU before any hardware.
- Keep `InstallationConfig` the installer's only contract.

## Non-goals (v1)

- **IP allocation from the index.** The fleet's addressing is arithmetic (`.92`/`.94`/`.96` = index × 2), which invites deriving `network_address`. It collides with DHCP and NIC replacement. `network_address` stays an explicit `HostProfile` fact.
- **CockroachDB-backed profile storage.** See Decision 4 — the daemon has no DB connection, and adding one is its own operation.
- **Continuous convergence / re-render on group change.** v1 resolves at deploy time. **Known gap (stated, not hidden):** when a 4th node joins, nodes 1–3 keep their already-rendered configs, and drift detection watches *profile objects*, not *rendered outputs* — nothing detects a stale rendered config. Tolerable at 5 machines because CockroachDB's `--join` matters only at first start and the cluster gossips thereafter. Keepalived would **not** tolerate it, which is why it is deferred.
- **HAProxy and Keepalived/VIP applications.** Deferred. Per the simplicity panel, the sibling *abstraction* they would need is deferred with them (see Decision 17) — v1 uses the existing `HostSpec::compute_join` directly.
- **Retiring the curtin/autoinstall path.** `autoinstall::` is NOT dead code — `uaa place`, `uaa verify`, `uaa render-user-data` are live CLI subcommands consuming it. Retirement removes three subcommands and is out of scope.
- **rpi-serv-001/002/003** — never go through `uaa install`.
- **Resistance to a server-root adversary** — inherited bound; see Decision 9.

## Decisions (locked during design)

1. **Three concepts, two tiers plus a workload axis.** `HostGroupProfile` (class defaults + naming scheme + default applications), `HostProfile` (per-machine facts + own applications), `Application` (workload, assignable at either level). Scalars/lists: override-wins. Application lists: **union** group ∪ host, host-level per-application fields overriding group-level. *Losing alternative:* one flat struct conflating hardware class and workload — rejected because `unimatrixone` shares the Lenovos' hardware/secrets model but runs no Cockroach, so the axes vary independently. (Panel: all three lenses CONFIRMED; the simplicity lens tried hardest to cut the Application axis and could not — collapsing it forces a `has_cockroach: bool`, and HAProxy/Keepalived make it a third and fourth boolean.)
   **The merge engine emits per-field provenance** (`field -> group | host | default`) alongside the resolved config, so "why does this host run Cockroach?" is answerable without mentally recomputing two JSON blobs. The engine already knows; it must say.
   **Per-application override needs an all-`Option` partial twin.** *Panel finding (correctness, CHALLENGED v1):* "the host-level copy's fields override the group-level copy's" is unimplementable against `CockroachSpec` as written — `seed_ip` has no default, so a host wanting to override only `locality` cannot supply a partial without also restating `seed_ip`; deserialization demands it. Either the semantics silently degrade to whole-application **replace** (contradicting the locked model), or each variant needs a partial twin. Locked: **`CockroachSpecPartial`** — every field `Option<T>` — merged field-by-field onto the group's `CockroachSpec`. It is ~20 lines for the one variant that exists and preserves the model the operator asked for. Each future variant ships its own partial twin.

2. **The group name is the hostname prefix; it is IMMUTABLE; and the real invariant is a GLOBAL hostname uniqueness check.** `len-serv` → `len-serv-{index:03}`. **A group cannot be renamed** — the same rule `standalone` already has.
   *Panel finding (correctness, CHALLENGED v1):* v1 claimed prefix uniqueness "is the only thing preventing two groups allocating the same hostname". **False** — `hostname_pattern` is free-form per group, so a group `len` with pattern `{name}-serv-{index:03}` and a group `len-serv` with the default pattern both render `len-serv-001`; and `hostname_override` can collide with any generated name. Prefix uniqueness is necessary but **not** sufficient. The load-bearing invariant is: **every materialized hostname is globally unique across all groups**, enforced at allocation and validated against `hostname_override` too. Prefix uniqueness stays as a cheap early error with a better message. *Panel finding (ops, CHALLENGED v1):* v1 keyed allocations on `group_name`, a mutable string, while `HostGroupRow` carried an unused stable `id`. Renaming `len-serv` → `len-server` would orphan every allocation under the old name, restart allocation at index 1, and mint `len-server-001` while the live `len-serv-001` still holds that address — **the exact bug this design exists to prevent, through a different door.** Fixed two ways, belt and braces: allocations key on the immutable `group_id: Uuid`, **and** renaming is forbidden. Renaming is achieved by creating a new group and rebinding hosts (Decision 18).

3. **A permanent, undeletable `standalone` group** for one-offs. It does not auto-name: `hostname_override` is required there. *Panel finding (simplicity, CHALLENGED v1):* v1's "warn when a second host joins standalone" fires on the fleet's **normal** state — `vm-test` and `unimatrixone` are 2 of 5 machines and both belong there. A warning that is always on is noise. **Cut.** Undeletable + explicit-hostname remain.

4. **Profiles persist in the existing `StatePaths` snapshot, NOT CockroachDB.** *Panel finding (simplicity + ops, CHALLENGED v1 — the finding that overturned the architecture):* **`uaa-control` has no database connection in production.** Verified independently: `tokio_postgres` appears in no wiring file; `default_state()` (`operator/handlers.rs`) — the only reachable production state builder, since `operator::router()` takes no arguments — constructs `FileRegistry(StatePaths)` + `MemEnrollmentStore` + `MemAuditStore`; `db::migrations::apply` has **no caller**; and the crate's own module doc admits wiring `PgAuditStore` "would need DB connection plumbing this crate's `main.rs`/`listeners::serve` doesn't have today." v1's four CRDB tables would have been created by nobody and read by nobody, and profiles would have fallen back to an in-memory store and vanished on every restart — precisely the failure v1's Decision 11 existed to prevent.
   The de-facto durable production store is the `StatePaths` JSON snapshot that `machine_plane::{seeds,lifecycle,inventory}` and `dashboard` already share. Profiles use it. This still satisfies the locked user decision (**control-managed, not git-backed**) — the drift/accept/revert argument against git is untouched, and a service still watches the store.
   Two further arguments the panel did not make, recorded because they are load-bearing: **(a) bootstrap circularity** — CockroachDB runs *on the machines this system deploys* (`COCKROACH_SERVER_IP` is the seed), so storing the deployer's source of truth in the cluster it deploys is a deadlock risk that does not exist today; **(b) no restore path** — `main.rs`'s `Import`/`Export` are `not_yet_implemented(...)` → `exit(1)`, so a DB-backed store would have no backup route, whereas the snapshot is a file that `cp` backs up. *Losing alternatives:* CRDB (unwired, unscoped, circular); git-backed YAML (no service can watch it for drift).

5. **New sibling `ProfileStore` trait + module — do NOT extend `RegistryStore`.** `RegistryStore` is a 14-method monolith with two impls, and `saga.rs`'s `RecordingRegistry` hand-forwards **every** method — growing the trait silently breaks a file outside this work's scope. `saga.rs`'s `SagaStore` + Pg/Mem twins is the established separate-trait precedent. (Panel: CONFIRMED by all three lenses; ops added that a disjoint trait means rolling this work back cannot regress `RegistryStore` consumers.)

6. **Row types live in `db/mod.rs`; profile collections are `SnapshotDoc` fields.** `db/mod.rs`'s module doc states row types are "pre-declared HERE" so followers never redefine them. `SnapshotDoc`'s six existing collections are each `#[serde(default)]`, so **adding profile collections is backward-compatible with every snapshot file already on disk**. No migration, no `SQL_*` consts, no `0002_profiles.sql`, no `TABLES` count bump — all of v1's Decision 6 is deleted with Decision 4. Every mutation follows the store's stated CONTRACT: `write_snapshot` after every committed mutation, via `guarded_mutation`.

7. **Allocation is insert-if-absent, and the no-clobber law is preserved in spirit.** Decision 22's `ON CONFLICT DO NOTHING`-never-`DO UPDATE` law was SQL-shaped; under Decision 4 it restates as: **`allocate_index` never overwrites an existing `(group_id, identity)` binding.** A second allocate for a bound identity is a no-op returning the existing index. This is what makes allocation idempotent and re-derivation impossible. *Panel note (ops, on the v1 SQL form):* v1's conflict target `(group_name, identity)` did not cover the separately-`UNIQUE` `index`, so two concurrent allocations computing the same next index would have errored rather than no-ops. Under Decision 4 this becomes a single-writer concern: allocation runs inside `guarded_mutation`, which serializes it; concurrent allocators are serialized by the same lock, so the next-free-index computation cannot race.

8. **⚠ Allocation reads FAIL CLOSED. This is the single most important safety property in this design.** `read_snapshot` **fails open**: on a missing *or corrupt* snapshot it logs `"serving EMPTY registry (degraded)"` and returns `SnapshotDoc::default()`. That is correct for telemetry ingest and **catastrophic for allocation** — an allocator reading an empty view sees zero bindings and re-allocates every index from 1, **renaming the entire fleet**. This is the design's core bug reachable through a third door, and it is a property of code that already exists.
   Therefore: allocation and hostname resolution MUST NOT read through the plain fail-open path. They use a distinct `read_snapshot_strict(paths) -> Result<SnapshotDoc, StoreError>` that returns `Err` on missing-or-corrupt, and every allocation caller refuses to allocate on `Err`. `MemProfileStore` is `#[cfg(test)]`-gated so the wrong wiring cannot compile. **Never** `unwrap_or_default()` a snapshot read on an allocation path.

9. **The drift threat model is inherited and bounded, not extended.** Per Decision 21b (`audit.rs`, verbatim): the chain "defends against a rogue operator without server root… A server-root adversary defeats it — they can rewrite the CockroachDB rows and the on-disk signing key together, and nothing in this module claims otherwise." Drift detection inherits exactly this bound. (Panel, ops: since `content_hash` is stored beside `body`, any writer who can edit one can edit both — so drift detection's real yield is **accident and mistake detection**, not defense. Stated plainly rather than oversold. Snapshot-file storage makes accidental hand-editing *more* plausible than a database would, which is what makes it worth having.)

10. **The drift fingerprint is a NEW mechanism, distinct from the audit chain.** `audit.rs` chains *events* (`hash = SHA-256(prev_hash ‖ canonical_bytes(event))`) — that answers "was the event log tampered with", not "did this object's body change out-of-band". Drift needs a per-object `content_hash = SHA-256(canonical_bytes(body))`, reusing the BTreeMap-sorted-keys canonicalization *idea* as a separate function. Both are kept: the chain records **who** changed a profile through the API; the hash detects a change that **bypassed** it. (Panel: CONFIRMED by all three; simplicity tried hardest to fold it into the audit chain's `detail` field and could not — the audit store is `MemAuditStore` in production and does not survive restart, so it cannot be a revert source.)

11. **Revert restores the last GOOD version — not "N−1" — and it restores INTENT, not the machine.** No revert precedent exists in the crate; the only review flows are binary and terminal. *Panel finding (correctness, CHALLENGED v1 — the deepest bug the panel found):* v1 said "write a new version whose body equals version N−1". That is right for *undo my last change* and **wrong for drift repair**, and v1 reused one rule for both. Two failure modes: **(a)** if the drifted live row is still labeled version N, reverting to N−1 **silently discards the last legitimate change** along with the drift; **(b)** if `profile_versions` holds only *prior* versions, then version N's good body was **never captured** — the out-of-band edit overwrote the sole copy and revert cannot reconstruct it at any price. v1 rejected in-place mutation because "it destroys the drift evidence", then specified a revert that destroys it just as thoroughly (accept preserves the drifted body; v1's revert did not).
   **Locked, three parts:** (i) append the current body to `profile_versions` on **every** API write, so version N is always recoverable; (ii) on detecting drift, capture the **drifted** body as its own version row (marked `source: "drift"`) *before* any review action, so accept and revert both preserve the evidence; (iii) define drift-revert as **"restore the newest version whose `body` still hashes to its own stored `content_hash`"** — the last provably-untampered version — never a blind N−1.
   Revert stays forward-only (a new version, nothing destroyed). *Panel finding (ops):* v1 also implied revert fixes something real. It does not — v1 has no re-render (Non-goals), so revert changes a stored row and **leaves the deployed machine exactly as drifted as it was**. The operator must not read "reverted" as "fleet fixed"; re-deploying is a separate explicit action. This wording is normative and must appear in the UI, not only here.

12. **Drift detection is scheduled, not incidental.** *Panel finding (ops, CHALLENGED v1):* v1 defined drift as "*a read* that finds `stored.content_hash != content_hash(stored.body)`" — lazy and read-triggered, so drift to a profile nobody reads is never surfaced. v1 also had nothing to stop an out-of-band editor and the revert button thrashing forever. v2: a periodic scan walks every profile object, emits a counter/log line per drifted object, and repeat drift on the same object within a window is reported once with a count, not once per scan.

13. **Application install folds into Phase 5 — no new numbered phase.** `PhaseSelection` hardcodes `0..=6` in three places. *Panel note (ops):* the real, unpriced cost is that `PhaseSelection` is the operator's only granular retry lever — folding applications into Phase 5 means a failed Cockroach install cannot be retried alone; you re-run all of Phase 5 (repeating every other Phase-5 mutation) or reinstall. Accepted deliberately: widening the phase range touches three hardcoded sites and every call site, for a retry ergonomic on a 5-machine fleet. **Recorded so it is not discovered at 3am.**

14. **Application install is a separate module, deliberately fail-closed.** `applications.rs` mirrors `ResetPartitionStager`'s *shape* (`new(runner)`, one primary async fn) but **not** its error handling: the reset stager is non-fatal by design; an application failing to install is a **failed deployment** and propagates with `?`. *Recorded consequence (ops):* Phase 5 runs *after* the disk is LUKS-formatted and the OS installed, so a Cockroach failure aborts with the target **installed but unconfigured**, recoverable only by reinstall or a full Phase-5 re-run. Tolerable; written down rather than discovered.

15. **`applications` is an additive, defaulted, and skip-serialized `InstallationConfig` field.** `#[serde(default)]` is mandatory: `InstallationConfig` carries `deny_unknown_fields`, has no `Default` impl, and every construction is an exhaustive struct literal (4 sites, one on a live CLI path). *Panel finding (ops):* additionally mark it `#[serde(skip_serializing_if = "Vec::is_empty")]`. Rationale: once M4 re-authors placed configs from a serialized resolved config, a host with no applications serializes **without** the key — so a rolled-back `uaa install` binary (which does not deploy in lockstep with control, and enforces `deny_unknown_fields`) still parses it. Without this, rolling back control leaves every PXE-ing machine failing a fail-closed parse on a file the rollback didn't touch. (Panel verified v1's additivity claim is otherwise true: `place_configs` never re-serializes today — it does line-based textual injection on the source YAML and writes that text verbatim.)

16. **`Application` is a closed-but-growing Rust enum**, not a plugin trait. *Recorded rollback constraint (ops):* once a later wave adds a `HaProxy` variant and a profile row persists it, that row is **unparseable by a rolled-back older control** (unknown tag). The enum makes code-rollback fail-closed against stored data — correct, but it means later waves are **roll-forward-only once a new variant is persisted**.

17. **Sibling resolution uses the existing `HostSpec::compute_join` directly; the `ResolvedSibling` abstraction is DEFERRED.** *Panel finding (simplicity, CHALLENGED v1):* v1 called `ResolvedSibling`/`siblings()` "the load-bearing abstraction", but `compute_join(server_ip, members: &[&str], self_ip, port)` **already takes plain IP strings and already filters self**. Cockroach needs only a `Vec<String>` of member IPs; `index`/`hostname` on `ResolvedSibling` serve only the deferred Keepalived, and `siblings()` re-implements the self-exclusion `compute_join` performs. It was load-bearing for a non-goal. v1 shipped the abstraction; **v2 ships the caller**. Introduce the sibling type in the Keepalived wave that justifies it.

18. **`rebind` exists, and is the one deliberate exception to append-only.** *Panel finding (ops, on D-A):* a NIC dies on `len-serv-002` and is replaced → new MAC → new identity → no allocation row → allocate-if-absent mints the **next free index** → the machine returns as `len-serv-004`. Worse, since indices are never reused, index 002 is **permanently burned** and `len-serv-002` can never exist again. Hardware replacement is a normal event, not an exception. Therefore: `rebind(group_id, old_identity, new_identity)` — audited via `AuditStore::append_in_txn`, operator-gated at `Role::Operator`, moving the existing index+hostname to the new identity and tombstoning the old row. It deliberately violates append-only **once, under audit**, because the alternative is bricking an index on every NIC swap.

19. **Naming: `HostProfile`/`HostGroupProfile`, never bare `profile`.** `BuildIsoRequest.profile` is an ISO-build variant selector — confirmed to carry no host or MAC field and to be structurally unrelated.

## Resolved: the two panel-routed questions

### D-A. Durable machine identity → **A1 (MAC) + `rebind` (Decision 18)**

No field available today is both stable across reinstall and non-spoofable:

| Candidate | Stable across reinstall? | Non-spoofable? | Available today? |
|---|---|---|---|
| MAC | Yes (`reinstall.rs` never touches identity fields) | **No** — code comments call it spoofable | Yes — the only cross-request key |
| TPM EK | Yes | Yes | **Partly** — optional/best-effort; **never checked by `enroll.rs`'s CSR flow** |
| Enrollment SPKI fingerprint | **No** — `enroll.rs`'s own test asserts it changes | Yes | Yes |
| Minted UUID on target | No (wiped by reinstall) | Yes | No |

**Locked: A1 + `rebind` (Decision 18). No EK story is claimed.**

The requirement is *allocate-once-never-re-derive*, which is orthogonal to spoofability — binding index→MAC and never recomputing already fixes the stated bug (a machine with a lower MAC added later gets the next free index, because allocation is append-only rather than sort-derived). Both the correctness and simplicity lenses confirmed that argument holds. Machine identity is already MAC-keyed all the way down (`mac_for_host`, the `/var/www/html/cloud-init/<hexmac>/` webroot path, the registry), so a different key would be a lone island.

**A2** (mandatory-EK enrollment) drags an enrollment redesign in for zero fleet benefit, and a TPM-less QEMU gate could not allocate at all.

**A3 is rejected, and v2.0's rationale for waving at it was wrong.** v2.0 claimed the EK-mismatch alarm "exists and costs nothing extra" because `/api/checkin` binds `tpm_ek` and 403s on mismatch. *Panel finding (correctness):* the 403 shape is real, **but nothing on the native install path ever posts an EK** — `grep -n 'checkin\|tpm_ek\|tpm2_readpublic'` over `crates/uaa-core/src/network/` returns **no matches**; the only code that posts `tpm_ek` is the **retiring** curtin template's rc.local. So for exactly the machines this spec provisions, `tpm_ek` stays NULL, "record EK when present" never fires, and the alarm is **unreachable**. A3 is therefore not "free" — it requires wiring EK capture into the native install path, which is new, unscoped work. Claiming it without that wiring would be claiming a defense that does not exist.

**Locked: MAC as the allocation key; no EK capture, no EK alarm, and the spoofability bound documented (Decision 9 already declines that threat model); `rebind` (Decision 18) supplies the NIC-replacement runbook A1 otherwise lacks.** EK-bound identity is a **Non-goal (v1)** — recorded as the natural upgrade path, not shipped as a claim.

### D-B. Resolution locus → **B1 (resolve in `uaa-control` at place time)**

Control is already the only component that knows every sibling's IP; the installer stays dumb and its contract (one flat `InstallationConfig`) is unchanged; secrets injection stays server-local where it already is. *Panel (correctness) added the decisive argument:* B2 would require the installer to fetch profile data **before the machine is enrolled** — i.e. over an unauthenticated channel, which is *precisely* the plain-HTTP fetch-and-exec that C4 exists to remove. B2 also breaks `--inject-from`: secrets are injected server-side into a 0600 staging copy at place time, and the installer has no path to them.

*Correction to v1's reasoning (correctness):* v1 justified baking `--join` at place time by claiming it "matters at first start and the cluster gossips thereafter". **`--join` is read on every node start, not only the first.** The design survives anyway, for a different reason: `compute_join` always puts the seed first, so every node's join list contains `172.16.2.30` and the member union stays connected — a 4th node joining does not invalidate nodes 1–3. The conclusion held; the stated reason did not, and a brief citing the wrong reason would mislead.

*Recorded objection (ops):* B1 puts the profile store directly in the `config place` path — the one path that must work while rebuilding the fleet. Decision 4 (snapshot, not CRDB) substantially defuses this: the store is a local file on the same host, with no network dependency and no cluster to be down.

## Data model

```rust
// crates/uaa-control/src/db/mod.rs — row types live here by crate convention.

/// A machine class. `name` IS the hostname prefix and is IMMUTABLE (Decision 2).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostGroupRow {
    pub id: Uuid,                    // stable; allocations key on THIS, never on name
    pub name: String,                // unique; the hostname prefix; immutable
    pub hostname_pattern: String,    // default "{name}-{index:03}"
    pub is_standalone: bool,         // exactly one row true; undeletable
    pub defaults: serde_json::Value, // partial InstallationConfig
    pub applications: serde_json::Value,
    pub content_hash: Vec<u8>,       // serde_bytes_hex, per db/mod.rs convention
    pub version: i64,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
}

/// One machine's irreducible facts. Deleted with its group (Decision: cascade).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostProfileRow {
    pub id: Uuid,
    pub group_id: Uuid,
    pub identity: String,                  // the MAC (D-A: A1), normalize_mac'd
    pub hostname_override: Option<String>, // required iff group is standalone
    pub overrides: serde_json::Value,
    pub applications: serde_json::Value,
    pub content_hash: Vec<u8>,
    pub version: i64,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
}

/// Append-only index binding. NEVER deleted, NEVER cascaded (Decision 8/18).
/// The asymmetry with HostProfileRow IS the mechanism: profiles are deletable,
/// bindings are not, so deleting and recreating a group re-attaches every
/// machine to the index it already had.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostnameAllocationRow {
    pub group_id: Uuid,               // (group_id, identity) is the key
    pub identity: String,
    pub index: i64,                   // unique per group_id
    pub hostname: String,             // materialized at allocation time
    pub allocated_at: Option<String>,
    pub released_at: Option<String>,  // soft release; index never reused
    pub rebound_to: Option<String>,   // tombstone set by rebind (Decision 18)
}

/// Immutable prior versions — what revert reads (Decisions 10, 11).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileVersionRow {
    pub id: Uuid,
    pub object_kind: String,   // "host_group" | "host_profile"
    pub object_id: Uuid,
    pub version: i64,
    pub body: serde_json::Value,
    pub content_hash: Vec<u8>,
    pub actor: String,
    pub created_at: Option<String>,
}
```

### Persistence

Four new `#[serde(default)]` collections on `SnapshotDoc` (`db/store.rs`) — `host_groups`, `host_profiles`, `hostname_allocations`, `profile_versions`. Existing snapshot files on disk stay readable unchanged. Reads on allocation paths go through `read_snapshot_strict` (Decision 8); mutations through `guarded_mutation` + `write_snapshot`.

**Why `hostname_allocations` is a separate collection rather than fields on `HostProfileRow`** (the simplicity panel's conditional cut): because `host_profiles` **cascade-delete with their group** and allocations **never do**. That asymmetry is the entire mechanism — it is what makes "delete and recreate a group" re-attach every machine to the index it already had. Folding the binding onto the profile row would delete it with the profile and re-allocate from 1 on recreate, which is the bug.

## Components

### C1. Profile schema + merge (`crates/uaa-core/src/profile/`)
Pure, no I/O. `merge(group, host) -> (InstallationConfig, Provenance)` — override-wins for scalars/lists, union for applications, plus per-field provenance (Decision 1).

**Fail-closed is scoped to exactly the 10 required fields**, not "any unset field". *Panel finding (correctness, CHALLENGED v1):* a literal fail-closed-on-unset merge **rejects configs that parse fine today** — `len-serv-001.yaml` omits `network_renderer` entirely and relies on the serde default, so the M2 gate would fail on field one. Nine fields carry `#[serde(default)]` (`network_renderer`, `tang_threshold`, `enroll_tpm2`, `tpm2_pcr_ids`, `expect_fido2`, `install_ca_cert`, `initramfs_type`, `tang_servers`, `ssh_authorized_keys`) and three are implicitly optional `Option`s (`debootstrap_release`, `debootstrap_mirror`, `tpm2_pin`).

The 10 genuinely required fields — merge errors, naming each one still unset after both tiers: `hostname`, `disk_device`, `timezone`, `luks_key`, `root_password`, `network_interface`, `network_address`, `network_gateway`, `network_search`, `network_nameservers`. Everything else falls back to its serde default, and provenance records `default`.

**⚠ `tpm2_pin` must be `Option<Option<String>>` in a partial.** *Panel finding (correctness):* with a plain `Option<String>`, "not overridden by this host" and "this host explicitly has no PIN" are indistinguishable — so a host meant to have **no** PIN would silently inherit the group's. `None` = inherit; `Some(None)` = explicitly no PIN; `Some(Some(p))` = this PIN. The same trap applies to any future `Option` field in a partial.

### C2. Validation (`crates/uaa-core/src/profile/validate.rs`)
Fail-closed, every error named:
- **Global hostname uniqueness** (Decision 2) — the load-bearing invariant. Checked across every group's materialized hostnames *and* every `hostname_override`.
- Prefix uniqueness across all groups (cheap early error, better message — necessary but not sufficient).
- Group-name immutability; `standalone` undeletable; `hostname_override` required in `standalone`.
- **Exactly one `is_standalone` group.** *Panel finding (correctness):* v1 stated this as prose. Under snapshot storage there is no partial-unique-index to lean on, so validation must enforce it or a second standalone group is representable.
- `hostname_pattern` contains `{index` and renders a DNS-legal label.
- Unknown application `kind` is a hard error. *Panel note (correctness):* with `#[serde(tag = "kind")]` this surfaces at **deserialization**, so a stored row carrying a since-removed variant makes the whole profile unreadable rather than producing a named validation error. Acceptable and fail-closed (Decision 16 already makes variant removal roll-forward-only), but the error must be caught and re-reported naming the object, not bubbled as a raw serde message.

### C3. Profile store (`crates/uaa-control/src/profiles/`)
`ProfileStore` trait + `SnapshotProfileStore` (production, reusing `StatePaths`) + `MemProfileStore` (**`#[cfg(test)]`-gated**, Decision 8). `allocate_index` is insert-if-absent under `guarded_mutation`, reading via `read_snapshot_strict`. `rebind` (Decision 18) is the one binding mutation.

### C4. Cockroach application (`crates/uaa-core/src/network/ssh_installer/applications.rs`)
`ApplicationInstaller` mirroring `ResetPartitionStager`'s shape, fail-closed (Decision 14), invoked from `phase_5_system_configuration` **after** `install_ca_cert_in_chroot` (the Cockroach node cert is fetched over HTTP and needs the trust anchor). Chroot commands copy the file-wide literal `chroot /mnt/targetos bash -lc '<cmd>'`.

Ports `setup_cockroachdb.sh` (retrieved from the netboot server 2026-07-16; **not in git** — reproduced verbatim in the TASK brief so no executor needs the server): arch-aware binary download, `cockroach` system user, node cert from the **existing** `/api/certs/<hostname>?ip=<ip>` endpoint, a `cockroach.service` unit, and `--join`/`--advertise-addr` from `HostSpec::compute_join` (Decision 17). Porting it also removes a plain-HTTP fetch-and-exec of an ungoverned self-deleting script from first boot.

**Two traps the join derivation must handle** (*panel, correctness*):
- **Strip the CIDR.** `compute_join(server_ip, members: &[&str], self_ip, port)` takes **bare IPs** and filters self **by IP**, but `network_address` carries CIDR (`172.16.3.92/23`). Feeding it unstripped yields `172.16.3.92/23:36357` and a self-filter that never matches — so the node would list *itself* in its own join string. `HostSpec::ip_without_cidr` already exists and must be applied to every member and to self. v1's C4 never mentioned it.
- **Exclude soft-released members.** A member with `released_at.is_some()` is decommissioned; without an exclusion rule it stays in every join string forever, pointing new nodes at a dead host.

> **Recorded discrepancy (harmless, do not "fix" silently):** the live `len-serv-003-variables.sh` carries `COCKROACH_JOIN="…30,…94,…92"` — members **descending**. `host_spec.rs`'s test pins **ascending**. Join order is functionally irrelevant to CockroachDB. The Rust ordering is canonical.

### C5. Drift detection + review (`crates/uaa-control/src/profiles/drift.rs`)
`content_hash(body)` = SHA-256 over an **explicitly canonicalized** rendering of the body. Drift = `stored.content_hash != content_hash(stored.body)`. A **periodic scan** (Decision 12) surfaces it. Accept (adopt the current body) and revert (restore the newest self-consistent version, Decision 11) are both forward-only writes through `AuditStore::append_in_txn` — **not** `record()`, which passes a no-op mutation and must never be used for something that also changes state.

**Canonicalize explicitly; do not lean on `serde_json`'s internals.** *Panel finding (correctness):* the hash's determinism today rests on two **unpinned** assumptions — (a) `serde_json`'s `preserve_order` feature staying **off** (so `Value::Object` is a `BTreeMap` and keys re-sort on parse), which is a global feature-unification hazard any dependency could flip; and (b) no float ever entering a body (`1.0` vs `1` round-trips differently). `audit.rs` defends only its **top level** with an explicit `BTreeMap<&'static str, Value>`; an opaque nested body has no such defense. Worse, `test_content_hash_is_canonical` is **vacuous** with `preserve_order` off — it passes without exercising what it claims to guard. Therefore `content_hash` recursively sorts keys into a `BTreeMap` itself and rejects float values outright, and its test feeds a deliberately **shuffled-key** input built so it would fail if canonicalization were removed.

### C6. Application check-in (`crates/uaa-control/src/machine_plane/`, `crates/uaa-core/src/app_status.rs`)
Mirrors `luks_sync`'s precedent exactly: `LuksSyncPayload { mac, records }` → `post_sync` → 2xx **and** `body.ok == true`. The machine plane is deliberately fail-open and never touches CRDB; application status follows the same snapshot+WAL path. `MachineStatus` is **not** extended — it is the registration lifecycle, and its `Unknown(String)` variant exists to preserve Python parity.

**Staleness is computed at read time, never written by a timer** (panel, ops). The machine plane writes health only on check-in and nothing flips a status on absence — so a machine whose Cockroach died, or whose NIC died, or which never booted, keeps its last-known-good health **forever** and the SPA renders it green. That is strictly worse than showing nothing: the dashboard would actively assert health for a dead box. Therefore the read path computes `Stale`/`Unknown` from `last_seen` against a threshold at render time — no reaper, no background job, ingest stays fail-open — and the SPA distinguishes *reported healthy* from *hasn't reported since T*.

### C7. Operator API + SPA (`crates/uaa-control/src/operator/`, `web/`)
Mirrors `build_router`'s convention exactly: a `Router` per minimum `Role`, each `.with_state(...)`, wrapped in `auth::require_role(router, Role::X)`, merged, Extensions layered once. Reads at `Viewer`, mutations at `Operator`. Mutating handlers take `Extension<auth::Session>` and pass `&session.login` as the audit actor. DTOs are hand-written `Serialize`-only structs in `api_types.rs` mirroring `web/src/api/types.ts`. The SPA is `rust_embed`-served with an index fallback, so screens are client-side routes only.
The existing `MachineRow` DTO already carries `consistent: boolean` — *"True when every provisioning layer for this machine agrees; false = drift"*. Profile drift must **align with that vocabulary**, not invent a second one.

## Migration / integration

Every milestone below M4 is additive: `applications: []` resolves to exactly today's behavior, and the five committed YAMLs keep parsing unchanged (Decision 15 — verified: `place_configs` does line-based textual injection and never re-serializes).

`config_place.rs`'s hardcoded `KNOWN_HOSTS`/`mac_for_host()` generalize **staged, not big-bang**: the store becomes the source of truth first, `KNOWN_HOSTS` remains the default path until M4's flag is explicitly flipped. `--inject-from` and the `REPLACE_AT_PLACE_TIME` hard gate are untouched — resolution produces the same placeholder-bearing config injection then fills. **The resolved text must still satisfy the line-based injection matchers** (`inject_secrets` keys on `<key>: REPLACE_AT_PLACE_TIME`; `inject_install_ca_cert` does an exact string match). It degrades fail-closed via the PLACEHOLDER hard gate — a deliberate property, stated rather than left to luck.

**Soft-release semantics (stated, because v1 left them incoherent).** *Panel finding (correctness):* v1's Rollback said "correct a bad allocation by soft-release", but under insert-if-absent a re-allocate for a released identity is a no-op that hands back the released row's index — so soft-release corrected nothing. Locked semantics: `released_at` marks a machine **decommissioned**. The index stays bound to that identity forever ("never reused" means never given to a *different* identity). If the same identity returns, `allocate_index` **clears `released_at` and returns the same index** — the machine comes back under its original name, which is the entire point of allocate-once. Correcting a genuinely wrong binding is `rebind` (Decision 18), never soft-release. Released members are excluded from application sibling lists (C4).

## Files modified

| File | Change |
|---|---|
| `crates/uaa-core/src/network/ssh_installer/config.rs` | `ApplicationSpec`/`CockroachSpec`/`CockroachSpecPartial`; `applications` field (default + `skip_serializing_if`); **add `PartialEq` to `InstallationConfig`'s derives** — the M2 struct-equality gate needs it and the struct derives only `Debug, Clone, Serialize, Deserialize` today |
| `crates/uaa-core/src/profile/{mod,merge,validate}.rs` | NEW: schema, merge + provenance, validation |
| `crates/uaa-core/src/network/ssh_installer/applications.rs` | NEW: `ApplicationInstaller`, Cockroach step |
| `crates/uaa-core/src/network/ssh_installer/installer.rs` | one call in `phase_5_system_configuration` |
| `crates/uaa/src/cli/commands.rs` | the live `InstallationConfig` literal gains `applications` |
| `crates/uaa-core/src/app_status.rs` | NEW: client-side application status reporter |
| `crates/uaa-control/src/db/mod.rs` | 4 row types |
| `crates/uaa-control/src/db/store.rs` | 4 `SnapshotDoc` collections; **`read_snapshot_strict`** (Decision 8) |
| `crates/uaa-control/src/profiles/{mod,store,alloc,drift}.rs` | NEW: `ProfileStore` twins, allocation + `rebind`, drift |
| `crates/uaa-control/src/operator/{handlers,api_types}.rs` | `/api/profiles`, `/api/drift` route groups + DTOs |
| `crates/uaa-control/src/machine_plane/lifecycle.rs` | application-status ingest |
| `scripts/vm-validate.sh` | fail Stage 6 on `degraded`; Cockroach readiness assertion |
| `examples/configs/install/vm-test.yaml` | single-node `CockroachSpec` so the gate exercises the path |
| `web/src/**` | profile + drift-review screens; staleness rendering |

## Milestones

- **M1 — Profile schema + merge + validate + provenance.** Pure `uaa-core`. Additive.
- **M2 — Store + allocation + rebind.** `SnapshotDoc` collections, `ProfileStore` twins, `read_snapshot_strict`, insert-if-absent allocation, `rebind`. Additive; nothing reads it yet. **Gate:** a resolved config equals each committed host YAML by **struct equality** (`InstallationConfig == InstallationConfig`), *not* byte equality — the panel correctly noted the YAMLs are comment-rich (`vm-test.yaml` opens with ~29 lines of header) and no serializer reproduces comments, so a byte-identity gate could never go green and would be quietly weakened at the moment it mattered.
- **M3 — Cockroach application + a VM gate that can actually fail.** **Gate fix is part of this milestone, not an afterthought:** `vm-validate.sh` currently accepts `systemctl is-system-running` returning `degraded` as PASS — and `degraded` is returned *precisely when units have failed*. So `cockroach.service` could fail outright and the gate would report success. It also asserts nothing about Cockroach, and `vm-test.yaml` has no application entry, so with `applications: []` the gate would install nothing and still go green. M3 adds: a single-node `CockroachSpec` to `vm-test.yaml`, a **readiness** assertion (`cockroach sql -e 'SELECT 1'` / health `?ready=1` — `is-active` alone is insufficient, since the process can sit `active` retrying a join it never completes), and fails Stage 6 on `degraded`.
- **M4 — Resolution wired into `config place`.** The one behavior-changing milestone. Gated by `--from-registry` (default **off**) **and `--dry-run` default-on**, emitting a resolved-vs-committed diff and a placed-file count before any write; the previous `uaa.yaml` is kept as `.bak` so revert is an inverse operation rather than a re-derivation.
- **M5a — Drift detection + periodic scan + accept/revert.** Additive.
- **M5b — Application check-in + read-time staleness + SPA screens.** Additive. (Panel, simplicity: v1's M5 bundled three unrelated features across three planes; split.)

## Rollback

M1–M3 and M5 are additive and dormant: `applications: []` is every committed host's value, so reverting restores today's behavior exactly.

**M4 is the one milestone whose rollback is NOT free, and v1's claim that it was is withdrawn.** `/var/www/html/cloud-init/<hexmac>/uaa.yaml` **is data M4 writes**, via in-place `fs::write` with no backup. Reverting the commit does not rewrite those files. Because `InstallationConfig` carries `deny_unknown_fields`, a rolled-back `uaa install` — which does **not** deploy in lockstep with control — would hit the now-unknown `applications` key and refuse to install: every PXE-ing machine failing a fail-closed parse on a file the rollback didn't touch. Two mitigations, both required: **(a)** Decision 15's `skip_serializing_if = "Vec::is_empty"` means application-free hosts serialize without the key at all, so only Cockroach hosts (which need the new binary anyway) carry it; **(b)** rolling back M4 means **re-running `config place` without the flag**, plus the `.bak` files — the inverse operation, named here because v1 did not name it.

`hostname_allocations` is the one collection with real data risk, and the correct policy is **roll-forward-only, deliberately**: leaving it in place is not laziness — dropping it destroys the index bindings this design exists to preserve. A bad allocation is corrected by `rebind` (Decision 18) or soft-release, never by deletion.

**Forward-looking (Decision 16):** once a later wave persists a new `Application` variant, rows carrying it are unparseable by a rolled-back older control. Later application waves are roll-forward-only once a new variant is persisted.

## Testing

| Test | Asserts |
|---|---|
| `test_merge_host_overrides_group` | host field wins; unset inherits |
| `test_merge_application_lists_union` | group ∪ host; host app config overrides group |
| `test_merge_provenance_reports_source` | each field reports group\|host\|default |
| `test_merge_fails_closed_on_defaultless_unset_field` | a defaultless unset field is a named error |
| `test_serde_defaulted_field_is_not_unset` | C1's default interaction |
| `test_resolve_struct_equals_committed_yaml` | resolved == each committed YAML by **struct equality** (M2 gate) |
| `test_duplicate_prefix_rejected` / `test_group_rename_rejected` | Decision 2 |
| `test_standalone_undeletable` / `test_standalone_requires_explicit_hostname` | Decision 3 |
| `test_allocate_index_is_idempotent` | second allocate returns the same index, writes nothing |
| `test_allocate_never_reuses_released_index` | released index not handed out |
| **`test_group_delete_does_not_cascade_allocations`** | **the core requirement** — delete + recreate, indices unchanged |
| **`test_allocate_refuses_on_missing_snapshot`** | **Decision 8** — allocation on a missing snapshot is `Err`, NOT an allocate-from-1 |
| **`test_allocate_refuses_on_corrupt_snapshot`** | **Decision 8** — same for corrupt |
| `test_rebind_moves_index_and_tombstones_old` | Decision 18 (NIC replacement) |
| `test_rebind_is_audited` | `append_in_txn`, actor recorded |
| `test_content_hash_is_canonical` | key order does not change the hash |
| `test_drift_detected_on_out_of_band_edit` | mutated body + stale hash ⇒ drift |
| `test_revert_writes_new_version_not_destructive` | revert appends; N−1 restored; N still readable |
| `test_cockroach_join_matches_host_spec` | derived join == `HostSpec::compute_join` (no second impl) |
| `test_applications_empty_is_todays_behavior` | `applications: []` ⇒ byte-identical install commands |
| `test_application_install_propagates_failure` | a failing app fails the phase (Decision 14) |
| `test_stale_checkin_renders_stale_not_healthy` | C6 — "no news" ≠ healthy |
| VM gate (M3) | Cockroach installs **and answers `SELECT 1`**; Stage 6 fails on `degraded` |

## Open questions (resolved — recorded for the plan)

1. ~~Extend `RegistryStore`?~~ → No; sibling `ProfileStore` (D5).
2. ~~Reuse the audit chain for drift?~~ → No (D10); both kept, different questions.
3. ~~New numbered install phase?~~ → No (D13); fold into Phase 5, retry cost recorded.
4. ~~Is `BuildIsoRequest.profile` the same concept?~~ → No (D19).
5. ~~Extend `MachineStatus` with health?~~ → No (C6); registration lifecycle only.
6. ~~Retire the curtin path first?~~ → No; live behind three CLI subcommands.
7. ~~CockroachDB-backed store?~~ → **No (D4)** — the daemon has no DB connection; snapshot-backed.
8. ~~D-A durable identity?~~ → **A1 (MAC) + `rebind`** (D18).
9. ~~D-B resolution locus?~~ → **B1** (control, at place time).
10. ~~Ship `ResolvedSibling`?~~ → No (D17); deferred with Keepalived.
