<!-- file: docs/specs/qemu-validation-plan.md -->
<!-- version: 1.0.0 -->
<!-- guid: 7ce3cbd1-0ad4-48ef-b7b7-550cca013535 -->
<!-- last-edited: 2026-07-09 -->

# QEMU/swtpm VM Validation Gate + LocalClient Tests — Implementation Plan

**Design spec:** [qemu-validation-design.md](qemu-validation-design.md) — its Decisions
section is LOCKED (greenfield bash harness, Linux host only, virtio `/dev/vda`, swtpm
socket TPM2, harness-is-the-gate, harmless-real-command LocalClient tests). Do not
reopen them here.
**Workstream:** `testing-gates` (2 tasks). Task briefs live in
`docs/agent-tasks/testing-gates/`; this plan maps 1:1 onto them.
**Gate for every step:** `cargo test --lib --offline` (baseline 237 passed) and
`cargo build --offline`; shell-script work adds `bash -n <script>`.

---

## Standing constraints (restated, non-negotiable)

- **The gate rule:** no hardware install attempt and **no len-serv-003 wipe** until
  `scripts/vm-validate.sh` passes. Never wipe/reimage/touch 172.16.2.30 ("the server");
  harness runs there use a scratch dir only and leave nginx, autoinstall-agent, the
  debootstrap cache, netboot root, and CockroachDB node4 untouched.
- **Secrets:** the VM config uses throwaway VM-only values; no real
  `luks_key`/`root_password`/`tpm2_pin` enters git, and no `REPLACE_AT_PLACE_TIME`
  placeholder may reach the install step.
- Workers stay in their worktree (`$REPO/.worktrees/<ws>-<slug>`, branch
  `agent/<ws>-<slug>` off `origin/main`) and never push/PR/merge — the coordinator owns
  all git. File headers bumped on every touched file.

## Wave order

Waves are GLOBAL across the install-ops operation (see the skeleton/collision matrix and
`docs/agent-tasks/ORCHESTRATION.md`). This workstream occupies:

| Global wave | Task | Why there |
|---|---|---|
| 1 | testing-gates/TASK-02 (localclient-tests) | no dependencies; single file `src/network/local.rs` collides with nothing |
| 2 | testing-gates/TASK-01 (qemu-swtpm-harness) | HARD dependency on `installer-robustness/TASK-01` (wave 1) — the partition-suffix helper must be merged or the harness fails in Phase 2 on `/dev/vdap1` |

Note the counter-intuitive order: TASK-02 executes *before* TASK-01. TASK-01's brief
must verify the wave-1 merge landed before starting (re-verify block includes
`grep -rn 'sdap' src --include='*.rs'` expecting the old buggy `sdapN` test assertions
to be GONE post-merge).

## Step 1 — LocalClient unit tests (wave 1)

**Brief:** `docs/agent-tasks/testing-gates/TASK-02-localclient-tests.md`
(src `todo:local-tests`, P2, effort M, Sonnet-class, depends on: none)

Scope (design spec C4): add a `#[cfg(test)] mod tests` to `src/network/local.rs` —
today the file has zero tests (`grep -c "cfg(test)" src/network/local.rs` outputs 0).
The mock seam for callers is the `CommandExecutor` trait
(`src/network/executor.rs:11`); these tests exercise the real `LocalClient` with
harmless commands only (`echo`, `true`, `false`, `cp` on tempfiles), because
`LocalClient` executes real `bash -c`. Pin the API asymmetry: `execute` /
`execute_with_output` → `Err(ProcessError)` on nonzero exit;
`execute_with_error_collection` → `Ok((exit, stdout, stderr))` even on nonzero.
Cover: `connect` no-op Ok, `check_silent` true/false, stdout capture,
stderr-preferred error text, `upload_file`/`download_file` tempfile round-trip,
`Default`. Bump the header of `src/network/local.rs` (version + last-edited, keep guid).

Gates:

```text
Run:      cargo test --lib --offline
Expected: 246+ passed; 0 failed (baseline 237 + ~9 new LocalClient tests; exact count per brief)
Run:      cargo build --offline
Expected: exit 0
Run:      cargo clippy --offline
Expected: exit 0, no new warnings
Run:      grep -c "cfg(test)" src/network/local.rs
Expected: 1 (module added)
```

## Step 2 — QEMU+swtpm VM validation harness (wave 2)

**Brief:** `docs/agent-tasks/testing-gates/TASK-01-qemu-swtpm-harness.md`
(src `todo:qemu-gate`, P1, effort L, Sonnet-class, depends on:
**installer-robustness/TASK-01** — hard)

Scope (design spec C1–C3): greenfield `scripts/vm-validate.sh` (stages 0–7: preflight →
workspace → boot remastered ISO in QEMU with virtio disk `/dev/vda` + swtpm socket
TPM2 → interrogate live rootfs → full `uaa` install against the VM disk → boot the
installed disk → assert LUKS unlock, `rpool`+`bpool` import, systemd multi-user →
print the VERIFY-ON-VM report) plus operator doc `docs/vm-validation.md`. The report
MUST answer both markers in `scripts/build-installer-image.sh`
(`grep -n "VERIFY-ON-VM" scripts/build-installer-image.sh` → 3 hits: lines ~25/72/81):
the exact stock-installer autostart unit name on 26.04 live-server, and
present/MISSING for each of `debootstrap sgdisk zpool cryptsetup dracut clevis` in the
live rootfs. Linux host only (the server 172.16.2.30 or any amd64 Linux box; macOS
refused at preflight — no KVM). Do NOT reuse `src/utils/vm.rs` (locked rejection). Do
NOT edit `scripts/build-installer-image.sh`. Both new files carry 4-line headers.

Gates (authoring-time — the agent writing the script cannot run a VM):

```text
Run:      bash -n scripts/vm-validate.sh
Expected: exit 0 (parses clean)
Run:      cargo test --lib --offline
Expected: same pass count as post-Step-1 baseline; 0 failed (no Rust touched)
Run:      cargo build --offline
Expected: exit 0
Run:      grep -c "VERIFY-ON-VM REPORT" scripts/vm-validate.sh
Expected: 1+ (report block emitted)
Run:      grep -n "uname -s" scripts/vm-validate.sh
Expected: 1+ hit (Linux-host preflight present)
```

Gate (operator-run, on a Linux host, after merge — this is THE gate):

```text
Run:      sudo ./scripts/vm-validate.sh --iso <remastered-iso> [--workdir <dir>]
Expected: "GATE: PASS" and exit 0; VERIFY-ON-VM REPORT names the observed autostart
          unit(s) and shows all six tools present (or names the gaps to fix in the
          image overlay before any hardware attempt)
```

Until that operator run prints `GATE: PASS`, hardware installs and any len-serv-003
work remain blocked.

## Exit criteria for the workstream

- [ ] `src/network/local.rs` has a `#[cfg(test)]` test module; `cargo test --lib
      --offline` green with the increased count; headers bumped.
- [ ] `scripts/vm-validate.sh` exists, `bash -n` clean, emits the VERIFY-ON-VM report
      structure, refuses non-Linux hosts, uses virtio (`if=virtio`) + swtpm.
- [ ] `docs/vm-validation.md` documents prerequisites, invocation, report reading, and
      the gate statement.
- [ ] Operator VM run recorded with `GATE: PASS` before any hardware attempt (tracked
      outside git; the plan's job ends at delivering the runnable gate).

See `docs/agent-tasks/testing-gates/README.md` for the task table and
`docs/agent-tasks/ORCHESTRATION.md` for the coordinator/worker wave protocol.
