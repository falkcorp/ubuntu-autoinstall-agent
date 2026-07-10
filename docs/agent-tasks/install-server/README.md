<!-- file: docs/agent-tasks/install-server/README.md -->
<!-- version: 1.0.0 -->
<!-- guid: 30252199-1550-4abe-a8c0-aaaa50394f1e -->
<!-- last-edited: 2026-07-09 -->

# Workstream — install server operability

Make the install server on 172.16.2.30 observable and make a successful uaa install actually flip its host to local-disk boot. Five tasks against the HTTP-service repo mirror (`scripts/autoinstall-agent.py`), the server-local placement script (`scripts/deploy-usb-configs.sh`), and `docs/netboot-autodeploy.md`. Scope, locked decisions (NO HTTP secret-write API; server-side flip-tuple widening; read-only endpoints only), data model, and failure modes are in the design spec `docs/specs/install-server-design.md`; step order and gates in `docs/specs/install-server-plan.md`. No Rust source changes anywhere in this workstream.

**Execution mode:** SERIAL WAVES (coordinator-driven) — trigger: TASK-01, TASK-02, TASK-03, and TASK-05 all edit `scripts/autoinstall-agent.py` (collision table row `scripts/autoinstall-agent.py`, 4 tasks). TASK-04 touches only `scripts/deploy-usb-configs.sh` and runs parallel-safe in wave 1.

| Task | Src id | Title | Priority | Effort | Tier | Wave |
|------|--------|-------|----------|--------|------|------|
| TASK-01 | todo:usb-report-flip | Auto-flip on USB/SSH install success: widen webhook flip statuses to include `success` + tolerate missing iPXE file | P1 | S | Haiku-class | 1 |
| TASK-02 | todo:usb-agent-serving | First-class agent-binary serving: `/api/health` reports uaa-amd64 presence; document `/var/www/html/uaa/` nginx path + deploy step | P1 | S | Haiku-class | 2 |
| TASK-03 | todo:install-server-extras | `GET /api/uaa-configs`: per-host placed-config inventory (hexmac, hostname, mtime, placeholder-free bool) — read-only | P2 | S | Haiku-class | 3 |
| TASK-04 | todo:place-time-secrets | `deploy-usb-configs.sh --inject-from <secrets.yaml>`: place-time secret injection server-locally (NO HTTP write API — locked decision) | P2 | M | Sonnet-class | 1 |
| TASK-05 | todo:install-server-extras | `GET /dashboard`: single-page HTML status view (registry, last events, placed configs, agent binary presence) | P3 | M | Sonnet-class | 4 |

## Wave table

Wave numbers are GLOBAL across the whole install-ops operation (the skeleton is authoritative); other workstreams occupy the same waves, and a wave only starts after the previous global wave fully merges and every open sibling rebases.

| Wave | Tasks | Prereq | Parallel-safe because |
|---|---|---|---|
| 1 | TASK-01, TASK-04 | none | disjoint files (`scripts/autoinstall-agent.py` vs `scripts/deploy-usb-configs.sh`) |
| 2 | TASK-02 | wave 1 merged + siblings rebased | shares `scripts/autoinstall-agent.py` with TASK-01 |
| 3 | TASK-03 | wave 2 merged + siblings rebased | shares `scripts/autoinstall-agent.py` with TASK-02 |
| 4 | TASK-05 | wave 3 merged + siblings rebased | shares `scripts/autoinstall-agent.py` with TASK-03; also reuses the `agent_binary_status` (TASK-02) and `collect_uaa_configs` (TASK-03) helpers |

## Ground rules

- **`scripts/autoinstall-agent.py` is a REPO MIRROR.** The live copy runs on 172.16.2.30 at `/var/www/html/cloud-init/scripts/autoinstall-agent.py`. Every task documents (and NEVER executes) the human deploy step: `scp` the mirror to the server + `sudo systemctl restart autoinstall-agent`. No task touches 172.16.2.30 or len-serv-003 in any way — code/docs only.
- **No secrets in git.** Committed configs carry `REPLACE_AT_PLACE_TIME`; no task introduces a real `luks_key`/`root_password`/`tpm2_pin` anywhere in the repo, and placement code keeps refusing placeholder-bearing files. New endpoints are read-only and NEVER return placed-config file contents.
- Build + test gate for every task in this workstream:
  ```bash
  python3 -m py_compile scripts/autoinstall-agent.py   # mirror parses
  cargo test --lib --offline                           # 237+ passed; 0 failed (untouched Rust stays green)
  cargo build --offline                                # exit 0
  ```
  TASK-04 adds `bash -n scripts/deploy-usb-configs.sh`.
- **Verify every file:line anchor with `grep` before editing** — line numbers in each brief are a starting point, not a guarantee. Zero-hit at execution time means STOP and report.
- File headers: bump `version` and `last-edited` on every file touched; keep existing guids.
- Workers stay in their assigned worktree and NEVER push/PR/merge — the coordinator owns all git.

## Collision / wave note

**TASK-01, TASK-02, TASK-03, and TASK-05 all edit `scripts/autoinstall-agent.py`.** They MUST run in different waves (each serialized after the previous merges + siblings rebase) — running them in parallel would produce a same-file merge conflict on every rebase cycle. TASK-04 (`scripts/deploy-usb-configs.sh` only) is the only parallel-safe task and shares wave 1 with TASK-01. No file in this workstream is touched by any other workstream.

See [ORCHESTRATION.md](../ORCHESTRATION.md) (one level up) for the coordinator + worker protocol.
