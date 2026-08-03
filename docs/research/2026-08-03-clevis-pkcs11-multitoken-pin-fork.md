<!-- file: docs/research/2026-08-03-clevis-pkcs11-multitoken-pin-fork.md -->
<!-- version: 1.0.0 -->
<!-- guid: 2b7f14ce-9a03-4d6b-8e52-c1806f3ad947 -->
<!-- last-edited: 2026-08-03 -->

# Forking `clevis-decrypt-pkcs11` so two PKCS#11 tokens can unlock one LUKS device

**Status: proven in userspace on the VM gate. Not yet boot-proven in an
initramfs, and not yet wired into the installer — see
[What is NOT proven](#what-is-not-proven).**

## Why

The unlock policy we want contains one nested `sss` group
`{"t":2,"pins":{"pkcs11":[nano,carriedA,carriedB]}}`: two of three PKCS#11
tokens, each with its own secret PIN, decrypted in a single unlock.

clevis 23 as shipped cannot do that. It has exactly two PIN channels and both
are unusable here:

| Channel | Why it fails |
|---|---|
| `pin-value=` in the PKCS#11 URI | Non-interactive and works, but the URI ends up base64url-encoded in the JWE protected header inside the LUKS2 metadata, recoverable **with no passphrase**. Measured; see [Evidence](#evidence) row P2-RED. |
| `/run/systemd/clevis-pkcs11.pin` | Serves exactly **one** share per unlock. Measured; see row C0. |

## The upstream mechanism, quoted

`clevis` 23-1, package file `/usr/bin/clevis-decrypt-pkcs11` (there is no
separate `clevis-pkcs11` package). Lines 116-131 of 134:

```bash
PIN_value=""
if ! PIN_value="$(clevis_get_pin_value_from_uri ${uri})"; then
    PIN_value=$(cat "${PIN_FILE}" 2>/dev/null || :)
fi

if ! jwk="$(pkcs11-tool --login --decrypt --input-file ${ENC} \
            -p ${PIN_value} ${module_opt} ${mechanism_option} ${slot_opt} 2>${ERR})" \
            || [ -z "${jwk}" ]; then
    cat "${ERR}" >&2
    echo "Unable to decrypt the JWK" >&2
    # TODO: Verify invalid PIN more accurately
    echo "Invalid PIN?" >&2
    exit 1
fi

rm -rf "${PIN_FILE}" 2>/dev/null || :
```

Three facts follow directly from that text:

- **The delete is success-only.** Line 131 is unreachable on failure: the
  script runs under `set -eo pipefail` and the failure branch `exit 1`s at
  line 128.
- **The second share gets an empty `PIN_value`.** `-p` is unquoted, so with an
  empty value `pkcs11-tool` consumes the next flag as the PIN's argument and
  dies with `error: invalid option(s) given` — the exact error we measured.
- **The file is written by `/usr/libexec/clevis-luks-pkcs11-askpin`**, line 128,
  from a single `systemd-ask-password` at line 125. That prompt is *per device*,
  not per share, and is re-issued by the surrounding retry loop up to
  `too_many_errors=3` times.

Both boot paths funnel through the same two files: the systemd path via
`clevis-luks-pkcs11-askpass` → `askpin`, and the dracut path via
`50clevis-pin-pkcs11/clevis-pkcs11-hook.sh` → `askpin -d -r`. Patching
`/usr/bin/clevis-decrypt-pkcs11` therefore covers both.

## Measured: `clevis-decrypt-sss` runs shares CONCURRENTLY

`/usr/bin/clevis-decrypt-sss` is a compiled ELF, not a script, and imports
`fork`/`pipe2`/`epoll_create1`. Confirmed empirically by replacing
`clevis-decrypt-pkcs11` with a wrapper that timestamps its entry and sleeps 3 s
before `exec`ing the real script, then decrypting a 2-of-3 pkcs11 JWE:

```
ENTER pid=10280 t=1785732492.499647348
ENTER pid=10281 t=1785732492.499647370
ENTER pid=10286 t=1785732492.509663641
WALL=4.05s     (3 shares x 3s: ~3s => PARALLEL, ~9s => SERIAL)
```

All three entered within 10 µs. This is why the fork holds a lock across the
prompt and the login: without it, N `systemd-ask-password` queries are
outstanding at once and **their timeouts run down together**, so prompt #2 can
expire while the operator is still typing #1. It is also why each prompt names
its token — the order is not deterministic (observed both `carriedA, carriedB`
and `carriedB, carriedA` across runs).

`clevis-decrypt-sss` also `kill`s its remaining children once the threshold has
become unreachable, which further bounds PIN burn.

## The patch

Two forked files, both in `clevis/` in this repo:

### `clevis/clevis-decrypt-pkcs11` (+85 / −8 against upstream)

**Fork 1 of 2 — slot resolution is mandatory.** Upstream fell back to
`slot_opt=""` when the URI did not resolve, letting `pkcs11-tool` pick whichever
token enumerates first. In an sss group of N pkcs11 shares that silently
collapses every share onto one token (rc=0, empty stderr), so a 2-of-3 group
unlocks with ONE token inserted. Failing here instead buys three things at
once: absence detection, the fix for that collapse, and the gate that stops us
prompting for a PIN for a token nobody inserted.

**Fork 2 of 2 — per-token PIN state.** The shared one-shot `PIN_FILE` is
replaced by state keyed on `sha256(serial|token)` taken from the share's *own*
URI:

- `/run/systemd/clevis-pkcs11.pin.<id>` — the PIN, mode 0600, tmpfs, cached
  after a successful login so `askpin`'s outer retry loop and any second LUKS
  device do not re-prompt. Deleted on a failed login so a bad PIN is never
  replayed.
- `/run/systemd/clevis-pkcs11.pin.<id>.tries` — prompt counter, capped at
  `CLEVIS_PKCS11_MAX_PIN_TRIES` (default **2**). Survives a failure so retries
  stay bounded.
- `/run/systemd/clevis-pkcs11.pin.lock` — `flock -w 300`, released before the
  final `jose jwe dec`. Best effort: if `flock` is missing the unlock still
  works, the prompts just interleave.

Prompting is `systemd-ask-password --timeout=${CLEVIS_PKCS11_PIN_TIMEOUT:-120}`
inside an `if !` guard, so a timeout fails closed instead of aborting the script
under `set -e`. `-p "${PIN_value}"` is now quoted.

The **retry cap of 2 is a deliberate trade**: a PIV token bricks after 3 wrong
PINs, so we stop at 2 and leave one. The cost is that after two typos the token
is unusable until reboot. That is recoverable; a bricked YubiKey on a machine
with no remote power is not. Raise `CLEVIS_PKCS11_MAX_PIN_TRIES` for tokens with
a larger counter.

### `clevis/clevis-luks-pkcs11-askpin` (−5 lines, pure deletion)

Upstream's single `systemd-ask-password` + `echo > /run/systemd/clevis-pkcs11.pin`
is deleted. With the decrypt-side fork prompting per token, leaving it would ask
the operator for a PIN that is then silently discarded — and block boot on its
90 s default timeout while doing so.

## Evidence

Ubuntu 26.04 guest `clevis-gate` (172.16.7.213) on U0, clevis `23-1`, three
SoftHSM2 tokens `nano` / `carriedA` / `carriedB` with PINs `111111` / `222222` /
`333333`. Negatives were run before positives. Judged on decrypted **content**
(`MARKER-52-OK`), never on exit code alone, and never via `clevis luks list`.

| Row | Setup | Expect | Result | Evidence |
|---|---|---|---|---|
| **C0** (confound) | **upstream** script, 2 tokens present, shared PIN file seeded | FAIL | **FAIL** rc=1 | `error: invalid option(s) given`; 2 `--login` calls. Proves the harness is not trivially passing and that the fork is what changes the outcome. |
| **N1** | fork, only `carriedA` present | FAIL | **FAIL** rc=1 | `PKCS#11 token not present / URI does not resolve to a slot` for `nano` and `carriedB`. **0** `--login` calls — no PIN burn for absent tokens. |
| **N2** | fork, 2 tokens, WRONG PIN on `carriedB` | FAIL | **FAIL** rc=1 | **2** `--login` calls (one per present token, exactly one wrong-PIN login). Cached PIN for `carriedB` REMOVED, `carriedA`'s retained. Stable over 2 runs. |
| **P1** | fork, 2 tokens, PINs seeded `222222` / `333333` | PASS | **PASS** rc=0 | `OUT=[MARKER-52-OK]`, content match, 2 `--login` calls. |
| **P3** | fork, 2 tokens, **interactive** `systemd-ask-password`, nothing seeded | PASS | **PASS** rc=0 | 2 prompts, each naming its own token, answered with two *different* PINs; 2 `--login` calls. Stable over 3 runs. |
| **P2-RED** | bind WITH `pin-value=` | PIN present in header | **PRESENT** | `111111`, `222222`, `333333` and the literal `pin-value` all recovered from `cryptsetup luksDump --dump-json-metadata` after recursive base64url decoding. No passphrase used. |
| **P2** | bind WITHOUT `pin-value=` | PIN absent | **ABSENT** | Same probe over the same 28 KB decoded blob: all four needles absent, plaintext and base64. |

Bounded-retry check: three consecutive interactive attempts with an agent that
always answers wrongly produced **4 prompts total across 2 tokens** — exactly
the cap of 2 per token — then stopped prompting.

Initramfs check: `dracut -f` rc=0 with 0 error lines; the initramfs copy of
`usr/bin/clevis-decrypt-pkcs11` has sha256
`e77854280744ddd2f05ec201335d39fb3f57e27a008addb03877d335bd4cf7db`, identical to
the fork on disk, and `usr/bin/flock` is present.

## Reconciliation baseline

Fork these from clevis **23-1** (amd64, Ubuntu 26.10 `stonking` pocket — never
`stonking-proposed` `23-1build1`, which drags in `libssl4`).

| File | Upstream sha256 |
|---|---|
| `/usr/bin/clevis-decrypt-pkcs11` | `63ef4486ea73122ed976b0a1c671f1200b54a501cb957d5d8dbbb4eb4d7d9081` |
| `/usr/libexec/clevis-luks-pkcs11-askpin` | `32396fe840bf4daf1e93176e3bfd6e5d823161358759770d7116552cb6bf2f73` |
| `/usr/bin/clevis-pkcs11-common` | `6c4fc20429d1651f89644acc1cb6b3367dd4445220e899c438dc0ca0ade6976c` |
| `/usr/libexec/clevis-luks-pkcs11-askpass` | `b1f725c3ac4189245e008778aa3fcfe0283a1755c68991f3f9e5a9f69db2218e` |

`clevis-pkcs11-common` is listed because the fork calls
`clevis_get_serial_from_uri`, `clevis_get_token_from_uri`,
`clevis_get_pin_value_from_uri` and `clevis_get_pkcs11_final_slot_from_uri` from
it. Drift there breaks the fork **silently** — a renamed helper returns empty
and every share collapses onto one PIN key. Treat a changed hash on that file as
a blocking reconciliation item even though we do not fork it.

**Reconciliation procedure on any clevis upgrade:** re-hash the four files;
for each changed one, `diff` the new upstream against the recorded baseline,
re-apply the two marked `UAA FORK` blocks onto the new upstream text, bump the
version headers, and re-run the row table above.

## Packaging (proposed, not implemented)

The vehicle already exists: `dracut/91uaa-keystore-wait/` is embedded with
`include_str!` and base64-piped into the target by
`install_keystore_dracut_module` (`system_setup.rs:983`). Mirror it exactly.

1. **Install.** A new `install_clevis_pkcs11_fork()` in `system_setup.rs`,
   called from the same place as `install_keystore_dracut_module()` and gated on
   `config.clevis_pkcs11_pin`, `include_str!`s both files from `clevis/` and
   writes them over `/mnt/targetos/usr/bin/clevis-decrypt-pkcs11` and
   `/mnt/targetos/usr/libexec/clevis-luks-pkcs11-askpin`, mode 0755. It must run
   **after** the apt install in `system_setup.rs:~525` (the package would
   otherwise overwrite the fork) and **before** the initramfs regeneration at
   `system_setup.rs:~721`.
2. **Initramfs.** No dracut-module fork is needed: `50clevis-pin-pkcs11/module-setup.sh`
   does `inst_multiple clevis-decrypt-pkcs11 /usr/libexec/clevis-luks-pkcs11-askpin`,
   which resolves to whatever is at those paths — so overwriting in place is
   sufficient, and it is verified above. `flock` is *not* in that list, so add a
   drop-in `/etc/dracut.conf.d/92-uaa-clevis-pkcs11-fork.conf` containing
   `install_items+=" /usr/bin/flock "` (config, not a third fork).
3. **Version stamping / drift detection.** Write
   `/etc/uaa/clevis-pkcs11-fork.json` recording the fork version, the four
   upstream sha256s above, and the clevis package version at install time. A
   dpkg trigger or a systemd unit re-hashes the installed clevis files and
   fails loudly when they no longer match — the drift signal that makes
   reconciliation mechanical rather than archaeological.
4. **Live host.** `clevis luks bind` runs on the live environment, not in the
   chroot (`system_setup.rs:1050`). Binding does not use `clevis-decrypt-pkcs11`,
   so the fork is only needed in the target. Do **not** install it on the live
   host.

## What is NOT proven

- **No boot-time proof.** Everything above is userspace on a running guest. The
  boot path adds dracut's `initqueue/settled` hook, the AF_UNIX control socket
  and a real console password agent. A boot row cannot be driven by the existing
  harness because nothing types PINs at the console; it needs console
  scripting against `virsh console`.
- **YubiKey retry-counter semantics are unproven.** SoftHSM has no PIV retry
  counter. What is measured is the number of `pkcs11-tool --login` invocations
  issued: **one per share per invocation**, capped at 2 prompts per token per
  boot. That the PIV counter decrements once per wrong login is assumed from the
  standard, not observed here.
- **Not wired into the installer.** Section [Packaging](#packaging-proposed-not-implemented)
  is a proposal; no Rust changed in this branch.
- **No hardware.** Nothing was bound or tested on `len-serv-*`, the RPis or U1.
- One early interactive run logged **3** logins instead of 2 (an extra login
  with the already-cached correct PIN). It did not reproduce across five
  subsequent runs. A duplicate login with a *correct* cached PIN does not
  decrement a PIV counter, so it is benign, but it is recorded here rather than
  swept up.
