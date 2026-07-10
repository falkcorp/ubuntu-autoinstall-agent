<!-- file: docs/agent-tasks/testing-gates/README.md -->
<!-- version: 1.1.0 -->
<!-- guid: b65fa13d-a27b-47cb-9bd7-13e56b2aa485 -->
<!-- last-edited: 2026-07-10 -->

# Workstream — testing gates (QEMU/swtpm VM validation + LocalClient tests)

Build the repeatable pre-hardware validation gate for the Path B installer: a greenfield
QEMU+swtpm harness (`scripts/vm-validate.sh` + `docs/vm-validation.md`) that runs the full
install against a virtio `/dev/vda` disk, resolves both `VERIFY-ON-VM` markers in
`scripts/build-installer-image.sh`, and asserts the installed system boots (LUKS unlocked,
`rpool`+`bpool` imported, multi-user reached) — plus a `#[cfg(test)]` unit-test module for
the entirely untested `LocalClient` (`src/network/local.rs`). Scope, locked decisions, and
stage semantics come from the spec: `docs/specs/qemu-validation-design.md` and
`docs/specs/qemu-validation-plan.md`. **THIS SCRIPT PASSING IS THE GATE — no hardware
attempt or len-serv-003 wipe before it passes.**

**Execution mode:** PARALLEL within global waves — trigger: 0 same-file collisions inside
this workstream (2 tasks, fully disjoint file sets; no testing-gates file appears in the
operation-wide collision matrix). TASK-01 sits in global wave 2 solely because of its hard
functional dependency on `installer-robustness/TASK-01` (the partition-suffix helper —
without it the VM install fails in Phase 2 on `/dev/vdap1`).

| Task | Src id | Title | Priority | Effort | Tier | Wave |
|------|--------|-------|----------|--------|------|------|
| TASK-01 | todo:qemu-gate | scripts/vm-validate.sh: QEMU+swtpm VM gate (virtio /dev/vda + TPM2) resolving both VERIFY-ON-VM markers - the gate before ANY hardware attempt | P1 | L | Sonnet-class | 2 |
| TASK-02 | todo:local-tests | Unit tests for LocalClient / local install flow (CommandExecutor seam) - today 0 tests exercise LocalClient | P2 | M | Sonnet-class | 1 |
| TASK-03 | ws10-gates | Constellation e2e VM gate: vm-validate-constellation.sh — loopback control/web/pxe + single-node cockroach + temp CA; enroll→approve→cert→install→verify sweep | P1 | L | Sonnet-class | constellation 8 |
| TASK-04 | ws10-gates | constellation-ci.yml: workspace clippy + test + SPA build check on PRs | P2 | S | Haiku-class | constellation 2 |

> **Constellation continuation (2026-07-10):** TASK-03/TASK-04 belong to the follow-on
> *constellation* operation (`docs/specs/constellation-design.md`); their Wave numbers are
> the constellation operation's GLOBAL waves, not the original install-ops waves used by
> TASK-01/TASK-02 above. TASK-03 is the **M5 gate** — THE gate before any hardware.

## Wave table

Waves are GLOBAL across the install-ops operation (see the operation collision matrix in
`docs/agent-tasks/ORCHESTRATION.md`). Note the counter-intuitive order: TASK-02 executes
*before* TASK-01.

| Wave | Tasks | Prereq | Parallel-safe because |
|---|---|---|---|
| 1 | TASK-02 | none | single file `src/network/local.rs` — appears in no collision row, disjoint from every wave-1 sibling (`install-server/TASK-01`, `install-server/TASK-04`, `installer-robustness/TASK-01`, `installer-robustness/TASK-02`, `installer-robustness/TASK-06`, `installer-robustness/TASK-08`) |
| 2 | TASK-01 | `installer-robustness/TASK-01` merged + siblings rebased | net-new files only (`scripts/vm-validate.sh`, `docs/vm-validation.md`, `examples/configs/install/vm-test.yaml`) — the wave-2 placement is a functional dependency (partition-suffix helper must be on `origin/main`), not a file collision |

### Constellation waves (2026-07-10 continuation)

Waves below are GLOBAL across the constellation operation (see
`.claude/state/plan-op-constellation-skeleton.json` `.global_waves` and the constellation
BREAKDOWN). Again counter-intuitive order: TASK-04 executes long before TASK-03.

| Wave | Tasks | Prereq | Parallel-safe because |
|---|---|---|---|
| constellation 2 | TASK-04 | constellation wave 1 (`core-proto/CP-01` workspace conversion) merged + siblings rebased | single net-new file `.github/workflows/constellation-ci.yml` — no testing-gates file appears in the constellation collision matrix |
| constellation 8 | TASK-03 | constellation waves 4–7 merged (`install-plane/IP-04` parity fixtures, `pki/PK-01`+`PK-02` enrollment, `uaa-web/WB-02` placement, `uaa-pxe/PX-01`) | net-new script `scripts/vm-validate-constellation.sh` + an append-only edit to `docs/vm-validation.md`; the wave-8 slot is purely functional (the services it launches must exist), not a file collision |

## Ground rules

- **Gate for every task** (run in the task worktree before reporting done):
  ```bash
  cargo test --lib --offline    # Expected: 237+ passed; 0 failed (TASK-02 raises the count)
  cargo build --offline         # Expected: exit 0
  ```
  TASK-02 (Rust) additionally runs `cargo clippy --offline`. TASK-01 (shell) additionally
  runs `bash -n scripts/vm-validate.sh` (and `shellcheck scripts/vm-validate.sh` if
  shellcheck is installed). The full VM run is TASK-01's *runtime* test — Linux-host-only,
  operator-run after merge; it is not part of the authoring gate.
- **Verify every file:line anchor with `grep` before editing** — line numbers in each brief
  are a starting point, not a guarantee. Zero hits at execution time = STOP and report.
- **File headers MANDATORY** on every file touched or created: bump `version` +
  `last-edited`, keep existing `guid` (new files get a new guid). Rust uses `// file:`
  comment lines; md/yaml/sh use the 4-line `<!-- file: -->` / `# file:` block.
- **HARD RULES for this workstream:**
  - NEVER wipe/reimage/touch 172.16.2.30 ("the server" — hosts nginx, autoinstall-agent,
    the debootstrap cache, netboot root, CockroachDB node4) or len-serv-003. All install
    validation happens inside a QEMU VM against a qcow2 scratch disk. DO NOT run installs
    against any physical host.
  - SECRETS: `examples/configs/install/vm-test.yaml` carries THROWAWAY VM-only values
    clearly labeled not-real. No real `luks_key`/`root_password`/`tpm2_pin` enters git,
    and no `REPLACE_AT_PLACE_TIME` placeholder may reach the install step.
  - `disk_device` in the VM config is `/dev/vda` because the harness *creates* that disk
    (`-drive if=virtio`); on real hardware the device is always READ from the live target,
    never guessed.
  - Workers stay in their worktree and NEVER push/PR/merge — the coordinator owns all git.

## Collision / wave note

**No testing-gates file appears in the operation collision matrix.** TASK-02 touches only
`src/network/local.rs`; TASK-01 creates only net-new files (`scripts/vm-validate.sh`,
`docs/vm-validation.md`, `examples/configs/install/vm-test.yaml`). Neither task shares a
file with the other or with any other workstream, so both are parallel-safe within their
global waves. The only ordering constraint is the hard dependency
`installer-robustness/TASK-01 → TASK-01` (that task's suffix-aware partition helper is
what makes the virtio `/dev/vda` install survivable — Phase 2/3 fail on `/dev/vdapN`
without it). TASK-01 must not start until that PR is merged to `origin/main` and this
workstream's worktrees are rebased.

### Constellation continuation (TASK-03 / TASK-04)

No testing-gates file appears in the constellation collision matrix either: TASK-04
creates only `.github/workflows/constellation-ci.yml`; TASK-03 creates only
`scripts/vm-validate-constellation.sh` and appends to `docs/vm-validation.md` (a file no
other constellation task touches). TASK-03 SOURCES helpers from `scripts/vm-validate.sh`
and never modifies it. Ordering constraints are purely functional: TASK-04 waits on
`core-proto/CP-01` (workspace layout); TASK-03 waits on constellation waves 4–7
(`IP-04`, `PK-01`, `PK-02`, `WB-02`, `PX-01` merged — the services the gate launches).
Constellation ground rules for these two tasks: TASK-04 additionally validates its YAML
(`python3 -c "import yaml; yaml.safe_load(open('.github/workflows/constellation-ci.yml'))"`);
TASK-03 additionally runs `bash -n scripts/vm-validate-constellation.sh` (+ shellcheck
if installed); the cargo gate baseline for the constellation era is 311 tests.

**Execution mode:** "SINGLE-AGENT for TG-03 (harness judgment); TG-04 anytime after
CP-01" — trigger: 1 task per local wave (`waves_local` = `[["TG-04"], ["TG-03"]]`);
0 same-file collisions inside or outside the workstream.

See [ORCHESTRATION.md](../ORCHESTRATION.md) (one level up) for the coordinator + worker protocol.
