<!-- file: docs/agent-tasks/uaa-web/TASK-04-publish-manifest.md -->
<!-- version: 1.0.0 -->
<!-- guid: fc6dc9b9-25b7-4ac7-a93f-20463cb61ff4 -->
<!-- last-edited: 2026-07-10 -->

# TASK-04 — Fill publish.rs: PublishAgentBinary verifying the detached sig BEFORE placement + update-manifest generation/serving (ws5-web)

**Priority:** P1 · **Effort:** M · **Recommended subagent:** Sonnet-class · rust-service subagent · **Why:** update-channel integrity; uaa-web never holds a signing key — verify-then-place ordering is the security property. · **Depends on:** TASK-01 (wave-7 gated: WB-01 merged — the `crates/uaa-web/src/publish.rs` stub must exist on `origin/main`) + CP-05 already merged in wave 3 (`crates/uaa-core/src/update.rs` manifest model + dual-pubkey verify)

## ⛔ START HERE (do this first, exactly)

```bash
REPO=/Users/jdfalk/repos/github.com/jdfalk/ubuntu-autoinstall-agent   # adjust to your clone
git -C "$REPO" fetch origin
git -C "$REPO" worktree add "$REPO/.worktrees/uaa-web-publish-manifest" -b agent/uaa-web-publish-manifest origin/main
cd "$REPO/.worktrees/uaa-web-publish-manifest"
git rebase origin/main
```

(Protocol also in `docs/agent-tasks/ORCHESTRATION.md` — the inline block above is authoritative for this task.)

## Goal

Fill `crates/uaa-web/src/publish.rs` (the ONLY file this task edits) per spec §C4 + §C7 and Decisions 9/10: `PublishAgentBinary` **verifies the artifact's detached ed25519 signature before placement — uaa-web never holds a signing key** (signing happens offline on the operator machine via `uaa release sign`, Decision 10; uaa-web only VERIFIES against its two embedded public keys), then atomically places the binary and the signed manifest under `<webroot>/uaa/` where the :8081 plane serves `manifest.json` + detached `.sig` (spec C7: `http://<uaa-web>:8081/uaa/manifest.json`, entries `{name, version, target, sha256, sig, url}` + global `min_version`). Purely additive within the one stub file.

REUSE — do not invent parallels for any of these:

- **Manifest model + verify helpers** from CP-05: `crates/uaa-core/src/update.rs` — `pub struct Manifest { pub binaries: Vec<BinaryEntry>, pub min_version: semver::Version }` and its dual-pubkey (`[VerifyingKey; 2]`, current + next, Decision 10) ed25519 verification. Do NOT define a second Manifest type or new verify code; re-read update.rs for the exact helper names (`grep -n "pub fn\|pub struct" crates/uaa-core/src/update.rs`).
- **Placeholder discipline:** the two embedded pubkeys come from build-time env (`UAA_UPDATE_PUBKEY`, Decision 10) or config; test keys are GENERATED in-test (`ed25519_dalek::SigningKey::generate`) — no real key material, public or private, is committed by this task.
- **Request/response types** from `crates/uaa-proto` (`proto/uaa/web/v1/web.proto` + `proto/uaa/update/v1/update.proto`, CP-02).
- **Atomic-write pattern:** same tmp+rename idiom as `placement.rs` (WB-02) — but implement your own private `write_atomic` in `publish.rs` rather than importing from the sibling stub (wave-7 siblings must not create cross-file edits; a shared helper hoist is a later cleanup).

**HARD RULES (non-negotiable):**
- NO hardware actions. Validate ONLY in-repo (`cargo`) and, where a brief says so, the QEMU+swtpm harness (`scripts/vm-validate.sh`). Code that COULD touch hardware is written and unit-tested against mock executors only.
- NEVER wipe, write to, or deploy on 172.16.2.30 ("the server") or len-serv-003.
- `disk_device` is read from the live target at runtime, never guessed or hardcoded.
- ipmitool runs via `ssh 172.16.2.30`, never on macOS.
- NEVER power on unimatrixone (U1).
- No real secret in any file: `REPLACE_AT_PLACE_TIME` placeholders stay placeholders.
- Stay inside your worktree; never `git push`, `gh pr`, or merge — report done and stop.

## Background (verify before editing)

- WB-01's `grpc.rs` already delegates `PublishAgentBinary` to this stub — replace the `unimplemented` body only; siblings TASK-02/03 own `placement.rs`/`iso_jobs.rs` this wave.
- The request carries: binary name + target triple + version + artifact bytes (or a staged path) + the artifact's detached ed25519 sig + the NEW manifest bytes + the manifest's detached sig. Both signatures were produced OFFLINE by the operator; uaa-web verifies both against its embedded `[VerifyingKey; 2]` — a signature valid under EITHER slot passes (rotation staging, Decision 10). Re-read `web.proto`/`update.proto` for exact field names.
- **Verify order — fail-closed at EVERY step, NOTHING written until all pass** (mirrors spec C7's verify chain; repeated in acceptance):
  1. manifest sig verifies (either pubkey) →
  2. manifest parses as the CP-05 `Manifest` type →
  3. manifest contains an entry matching the request's name+target+version →
  4. that entry's `sha256` equals the sha256 of the uploaded artifact bytes →
  5. artifact detached sig verifies (either pubkey) →
  6. anti-downgrade: the new manifest's version for this name+target is ≥ the currently-served manifest's version for the same name+target, AND its `min_version` is ≥ the current manifest's `min_version` (a replayed old-but-signed manifest must not downgrade — Decision 9 repair). No current manifest on disk → step 6 passes vacuously (first publish).
  Any failure → `Status::failed_precondition` (or `invalid_argument` for malformed input) with a reason naming the failed step, and the webroot is byte-identical to before.
- **Placement order matters:** artifact FIRST (`<webroot>/uaa/<name>-<target>-<version>` + its `.sig`), manifest + `manifest.json.sig` LAST — a :8081 reader must never see a manifest referencing an absent artifact. Each file lands via tmp+rename.
- Edge semantics: republishing an identical name+target+version with identical bytes → `Ok` idempotent (rename over same content); same version but DIFFERENT sha256 → `failed_precondition` (immutable releases).
- **Path map:** after CP-01 (wave 1) merges, `src/**` lives at `crates/uaa-core/src/**` and the CLI at `crates/uaa/src/**`. The greps below cite pre-move paths (verifiable on today's main); at execution time run them at the old path, then the mapped path. Zero hits at BOTH = STOP and report.
- **Re-verify these anchors before editing** — line numbers drift; zero hits at both old and mapped path = STOP and report:
  ```bash
  grep -n "min_version" docs/specs/constellation-design.md       # expect: 2+ hits (manifest floor, spec C7 + Decision 9)
  grep -n "never holds a signing key" docs/specs/constellation-design.md  # expect: 1+ hits (spec C4)
  grep -n "^ring\|^sha2" Cargo.toml                              # expect: hits (sha256 available; post-CP-01 check [workspace.dependencies])
  # Post-merge greps (files exist only after waves 1-6 — run at execution time):
  grep -n "fn publish_agent_binary" crates/uaa-web/src/publish.rs # expect: 1 hit (WB-01 stub you are filling)
  grep -n "pub struct Manifest" crates/uaa-core/src/update.rs     # expect: 1 hit (CP-05 model you reuse)
  grep -n "rpc PublishAgentBinary" proto/uaa/web/v1/web.proto     # expect: 1 hit (CP-02)
  ```

## Step-by-step

1. Run the ⛔ START HERE block, then every anchor grep above. Any zero-hit grep (at old AND mapped path) → STOP and report.

2. **Pubkey plumbing** — `pub struct PublishKeys(pub [ed25519_dalek::VerifyingKey; 2]);` loaded once at daemon startup (WB-01's config already carries `cert_dir`; add a `publish.rs`-local loader that reads the embedded/env keys the way CP-05's update.rs does — re-read update.rs and mirror its source-of-truth). `publish_agent_binary` takes `&PublishKeys` + `&WebConfig` + the request.

3. **Verification chain** — implement steps 1–6 from Background as a single `fn verify_publish(req, keys, current_manifest: Option<&Manifest>) -> Result<VerifiedPublish, String>` returning a typed proof object consumed by the placement step. Reuse the CP-05 verify helpers for both sig checks; sha256 via the existing `sha2`/`ring` workspace dep. NO filesystem writes inside `verify_publish`.

4. **Placement** — private `write_atomic` (tmp+rename, same dir); order: `<name>-<target>-<version>` binary → its `.sig` → `manifest.json` → `manifest.json.sig`. The manifest rename is LAST. Idempotent-republish and immutable-release checks per Background.

5. **Serving** — no new HTTP code: `/uaa` is already on WB-01's allowlist, so `manifest.json` + `.sig` are served by the existing :8081 plane the moment they land. Assert this in a test by writing via `publish_agent_binary` and reading through the WB-01 router (`tower::ServiceExt::oneshot` `GET /uaa/manifest.json`) if the router is importable without editing other files; otherwise assert the on-disk paths (do NOT edit `http.rs`).

6. **Unit tests** (`#[cfg(test)] mod tests`, tempdir webroot, in-test generated keypairs; a helper `fn signed_fixture(...)` builds a valid request):

   | Test | Asserts |
   |---|---|
   | `test_publish_happy_path` | **anti-over-suppression:** a correctly-signed artifact + manifest passes ALL six gates and lands: binary, binary `.sig`, `manifest.json`, `manifest.json.sig` all present with exact bytes; no `*.tmp.*` remains (the verify stack does not block a legitimate signed publish) |
   | `test_bad_manifest_sig_rejected` | manifest signed by an UNRELATED key → `failed_precondition`, tempdir byte-identical (zero new files) |
   | `test_bad_artifact_sig_rejected` | valid manifest, artifact sig from unrelated key → rejected, nothing written |
   | `test_sha256_mismatch_rejected` | manifest entry sha != artifact bytes → rejected, nothing written |
   | `test_manifest_missing_entry_rejected` | manifest lacking the name+target+version → rejected, nothing written |
   | `test_downgrade_rejected` | current manifest v1.2.0 on disk; new signed manifest carrying v1.1.0 for the same name+target → rejected, current manifest untouched |
   | `test_second_pubkey_slot_accepted` | sigs from the NEXT-slot key (slot 1) → accepted (rotation staging works) |
   | `test_manifest_written_last` | with a `write_atomic` hook/ordering probe (e.g. record rename order), the `manifest.json` rename is the final rename |
   | `test_idempotent_republish` | same request twice → second is `Ok`; same version different bytes → `failed_precondition` |

7. Bump the file header (version + last-edited) on every file you touch; new files get a fresh 4-line header with a new uuid4 (`uuidgen | tr 'A-F' 'a-f'`).

## How to test

```bash
cargo test --lib --offline && cargo build --offline
# Expected: all tests pass (wave-6 count + the 9 tests above), build clean
cargo clippy --offline -- -D warnings
# Expected: no warnings
cargo test --offline -p uaa-web publish
# Expected: 9 passed; 0 failed
grep -rn "SigningKey" crates/uaa-web/src/publish.rs | grep -v "cfg(test)\|mod tests"
# Expected: 0 hits — uaa-web never holds a signing key; SigningKey appears ONLY inside tests
git diff origin/main --stat
# Expected: ONLY crates/uaa-web/src/publish.rs (+ its header bump)
```

## Acceptance criteria

- [ ] Verify-before-place proven: `test_bad_manifest_sig_rejected`, `test_bad_artifact_sig_rejected`, `test_sha256_mismatch_rejected`, `test_manifest_missing_entry_rejected` all pass and each asserts ZERO new files in the webroot (`cargo test --offline -p uaa-web publish`).
- [ ] No signing key: `grep -rn "SigningKey" crates/uaa-web/src/publish.rs | grep -v "cfg(test)\|mod tests"` → 0 hits (verification only; test keys live inside `#[cfg(test)]`).
- [ ] Anti-downgrade floor: `test_downgrade_rejected` passes (Decision-9 `min_version`/version regression refused).
- [ ] Dual-pubkey rotation: `test_second_pubkey_slot_accepted` passes (Decision 10).
- [ ] Ordering + atomicity: `test_manifest_written_last` and the no-`*.tmp.*` assertion in `test_publish_happy_path` pass.
- [ ] Reuse: `grep -n "uaa_core::update\|update::Manifest" crates/uaa-web/src/publish.rs` → ≥1 hit (CP-05 model, no parallel Manifest type: `grep -c "pub struct Manifest" crates/uaa-web/src/publish.rs` → 0).
- [ ] **Anti-over-suppression:** `test_publish_happy_path` passes — a legitimately signed publish clears all six fail-closed gates end-to-end.
- [ ] Single-file scope: `git diff origin/main --stat` lists only `crates/uaa-web/src/publish.rs`.
- [ ] `cargo test --lib --offline && cargo build --offline` exits 0; clippy clean.
- [ ] File headers bumped on every changed file (`git diff origin/main --stat` files each show a version bump; guids unchanged).

## Commit message

```
feat(web): fill PublishAgentBinary — verify detached sigs before placement + manifest regen (ws5-web)

crates/uaa-web/src/publish.rs: six-step fail-closed verify chain (manifest
ed25519 sig -> parse -> entry match -> sha256 -> artifact sig -> anti-
downgrade version/min_version floor) reusing the CP-05 update.rs Manifest
model and dual-pubkey slots; nothing touches the webroot until every gate
passes. Placement is tmp+rename atomic with manifest.json renamed LAST so
:8081 readers never see a manifest referencing an absent artifact; idempotent
republish, immutable same-version releases. uaa-web holds verify keys only —
SigningKey exists solely in tests. 9 unit tests incl. signed happy path and
next-slot rotation.

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
```

## Done

STOP — report done with exact counts; the coordinator owns push/PR/merge.

## Idempotency / Rollback

Idempotency (additive — check for the NEW thing's presence): if `grep -n "verify_publish" crates/uaa-web/src/publish.rs` hits (the stub had only `unimplemented`), the task is already applied — run the acceptance checks instead of re-applying. Rollback = revert the single commit; the RPC returns `Unimplemented` again, `grpc.rs`, `placement.rs`, `iso_jobs.rs`, and uaa-core's update.rs stay untouched, and no published artifact or manifest exists anywhere (tests use tempdirs; the daemon was never deployed by this task).
