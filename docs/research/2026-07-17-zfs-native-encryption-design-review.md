<!-- file: docs/research/2026-07-17-zfs-native-encryption-design-review.md -->
<!-- version: 1.0.0 -->
<!-- guid: 307ecbc0-40ea-4eba-a110-41ded495b10c -->
<!-- last-edited: 2026-07-17 -->

# Adversarial Review + Design: ZFS Native Encryption Unlock on U1

Reviews [`2026-07-17-zfs-native-encryption-unlock-architecture.md`](2026-07-17-zfs-native-encryption-unlock-architecture.md)
(hereafter **the report**) and designs the replacement architecture for
`unimatrixone` (U1), Supermicro X10DSC+, Ubuntu 26.04 "resolute".

Grades: **PROVEN** (primary source quoted / source read at a pinned tag /
**executed**), **INFERRED** (reasoning from proven facts, shown so it can be
attacked), **COULD NOT VERIFY**.

> **New evidence class in this document: EXECUTED.** The report closes by
> saying "**Not verified by execution, anywhere in this report**". Several
> load-bearing questions were settled here *by running the actual shipped
> binaries on Ubuntu 26.04 with `zfs-2.4.1-1ubuntu5` and `clevis 20-1ubuntu2`* —
> the exact versions U1 will run — on `172.16.2.30`. U1 hardware was not
> touched, powered on, or contacted.

---

## TL;DR — the review found four things that change the design

1. **🔴 The report's own recommended design, built with the current
   `unimatrixone.yaml`, would HANG FOREVER on an unattended reboot.** A
   `systemd-tpm2` (PIN) token on the keystore is tried *before* the password
   path, needs a PIN, and loops `for(;;)` on an ask-password with **no
   deadline**. clevis never sees an ask file it recognises. Tang is never
   consulted. This lands precisely on U1's binding constraint. (§R4)
2. **🔴 The Tang enrollment config shape fails on a non-TTY channel, and a
   failure would be silent.** PROVEN BY EXECUTION: our exact SSS config shape
   fails with `/dev/tty: No such device or address`; a positive control proves
   the same environment succeeds once `thp` is pinned. `enroll_tang_clevis`
   then swallows any failure as non-fatal.
   ⚠️ **Scope, honestly:** what is proven is the *command shape*, in isolation.
   The Lenovos reportedly auto-unlock, which is evidence the full installer path
   succeeds *somehow* — so the correct reading is **"fragile and must be
   hardened"**, not "every host is broken". **One command settles it**
   (`cryptsetup luksDump` on a Lenovo, looking for a `clevis` token); I was
   unable to run it (no root). The recommended fixes hold either way. (§R2)
3. **🔴 The report prescribes `headless=` as the fix for its own identified
   fail-open. `headless=` would DESTROY the Tang path** — clevis works *by*
   answering the interactive prompt that `headless=` suppresses. It is also
   unreachable in the recommended design regardless. (§R5)
4. **✅ The OPEN QUESTION resolves in the design's favour, and the report's
   suspicion was wrong.** An unencrypted child under an encrypted parent is
   legal — no circularity. But the report **misdiagnosed** the real bug in the
   Ubuntu dracut port: it is a **zvol race**, not a layout error. (§R1, §R3)

Net: **native ZFS + LUKS keystore is still the right architecture**, but it must
be built with a *smaller* keystore token set than the report implies, a
mandatory dracut wait hook we ship ourselves, and Tang thumbprint pinning.

---

# PART 1 — ADVERSARIAL REVIEW

## R1. The OPEN QUESTION: unencrypted child under encrypted parent — **LEGAL**. PROVEN.

The brief asked me to determine this and distinguish local creation from
`zfs recv`. Both resolve, and **both resolve against the report's and the
brief's stated suspicions.**

### Local creation: allowed

`lib/libzfs/libzfs_crypto.c`, `zfs_crypto_create()`, tag `zfs-2.4.1`, read
directly (not paraphrased). The parent's crypt is fetched:

```c
/* Lookup parent's crypt */
pcrypt = zfs_prop_get_int(pzhp, ZFS_PROP_ENCRYPTION);
```

…and then, on the `OFF` path, **`pcrypt` is never consulted again**:

```c
	/*
	 * At this point crypt should be the actual encryption value. If
	 * encryption is off just verify that no encryption properties have
	 * been specified and return.
	 */
	if (crypt == ZIO_CRYPT_OFF) {
		if (proplist_has_encryption_props(props)) {
			ret = EINVAL;
			zfs_error_aux(hdl, dgettext(TEXT_DOMAIN,
			    "Encryption must be turned on to set encryption "
			    "properties."));
			goto out;
		}

		ret = 0;
		goto out;
	}
```

The kernel agrees — `module/zfs/dsl_crypt.c`,
`dmu_objset_create_crypt_check()`, verbatim:

```c
	crypt = (dcp->cp_crypt == ZIO_CRYPT_INHERIT) ? pcrypt : dcp->cp_crypt;
	...
	/* check for valid dcp with no encryption (inherited or local) */
	if (crypt == ZIO_CRYPT_OFF) {
		/* Must not specify encryption params */
		if (dcp->cp_wkey != NULL ||
		    (dcp->cp_keylocation != NULL &&
		    strcmp(dcp->cp_keylocation, "none") != 0))
			return (SET_ERROR(EINVAL));

		return (0);
	}
```

An explicit `encryption=off` sets `cp_crypt = ZIO_CRYPT_OFF`, so
`crypt == ZIO_CRYPT_OFF`, and the function **returns 0 without ever comparing
against `pcrypt`**. The `pcrypt == ZIO_CRYPT_OFF` rejection that does exist
lives only in the *inheritance* branch (`dcp->cp_wkey == NULL`), which an
explicit `encryption=off` never enters.

**⇒ `zfs create -o encryption=off rpool/keystore` on an encrypted `rpool`
is valid. There is no circularity. The report's gap #2 fear is unfounded.**
PROVEN.

### `zfs recv`: also allowed — the brief's belief is wrong for 2.4.1

The brief states: *"`zfs recv` of an unencrypted stream, which I believe IS
rejected."* It was, once. It is not now. `module/zfs/dmu_recv.c` at
`zfs-2.4.1`, verbatim:

```c
	} else {
		/*
		 * We support unencrypted datasets below encrypted ones now,
		 * so add the DS_HOLD_FLAG_DECRYPT flag only if we are dealing
		 * with a dataset we may encrypt.
		 */
		if (drba->drba_dcp == NULL ||
		    drba->drba_dcp->cp_crypt != ZIO_CRYPT_OFF) {
			dsflags |= DS_HOLD_FLAG_DECRYPT;
		}
	}
```

Commit **`68ddc06b6118`, 2022-02-09**, *"Receive checks should allow
unencrypted child datasets"* (closes #13033, #13076), whose message confirms
the history verbatim:

> "This seems like a remnant of the initial design, where **unencrypted
> datasets below encrypted ones weren't allowed**."

**⇒ Both halves of the OPEN QUESTION are permissive. The layout the Ubuntu
patch requires is constructible.** PROVEN.

## R2. 🔴 The Tang enrollment config shape fails on a non-TTY channel, and fails silently. PROVEN BY EXECUTION.

**This is the most immediately actionable finding, and the report missed it
entirely** — §7 credits us with "good instinct" for the `rd.neednet=1` GRUB
workaround, without ever checking that the enrollment which *produces* the Tang
keyslot can succeed on the channel the installer uses.

> **Read the grading carefully.** What is PROVEN BY EXECUTION is that *this
> config shape, on a non-TTY channel, fails*. The extrapolation to "therefore
> every installed host lacks a Tang keyslot" is **COULD NOT VERIFY** and is in
> tension with the Lenovos' reported behaviour. See "The Lenovo contradiction"
> below — I have deliberately not resolved that tension by assertion.

`system_setup.rs:781-784` builds:

```rust
let bind_cmd = format!(
    "clevis luks bind -d {} -k {} sss '{}'",
    luks_part, tmp_key_path, sss_config
);
```

No `-y`. And `enroll_tang_clevis` builds the SSS config with **no `thp` and no
`adv`** (`system_setup.rs:735-744`) — matching `unimatrixone.yaml`, which lists
bare `url:` entries.

The shipped `clevis-encrypt-tang` (clevis 20-1ubuntu2, read on the target) then
hits, verbatim:

```bash
### Check advertisement trust
if [ -z "${trust}" ]; then
    if [ -z "$thp" ]; then
        echo "The advertisement contains the following signing keys:" >&2
        ...
        read -r -p "Do you wish to trust these keys? [ynYN] " ans < /dev/tty
        [[ "$ans" =~ ^[yY]$ ]] || exit 1
```

`trust` is set only by `-y` (`trust=; [ -n "${2}" ] && [ "${2}" == "-y" ] && trust=yes`).
`clevis luks bind` propagates its own `-y` down (`clevis-luks-bind:148` passes
`"${YES}" ""`; `clevis-luks-common-functions:991` calls
`clevis encrypt "${PIN}" "${CFG}"`). Without `-y`, `YES` is empty.

**EXECUTED on 172.16.2.30 (Ubuntu 26.04, clevis 20-1ubuntu2, live Tang at
172.16.2.45/46/47):**

| Test | Config | Result |
|---|---|---|
| **A — our exact shape** | `{"t":2,"pins":{"tang":[{url}×3]}}`, no `-y`, non-TTY | **rc=1**, `clevis-encrypt-tang: line 120: /dev/tty: No such device or address`, **JWE = 0 bytes** |
| **B — positive control** | identical + `-y` | **rc=0**, JWE = 5499 bytes, `clevis decrypt` round-trips |

> **The negative test is validated.** Per
> `feedback_verify_the_test_before_trusting_the_result`: Test B proves the same
> host, same config, same Tang servers, same non-TTY channel **can** succeed.
> The Test A failure is therefore attributable to the missing flag, not to a
> confounded environment.

And the failure is **swallowed** (`system_setup.rs:793-800`):

```rust
if let Err(e) = bind_result {
    warn!("Clevis Tang enrollment failed (non-fatal — passphrase fallback remains): {}", e);
}
```

**⇒ If this bind fails, the installer reports success anyway, and the host
silently cannot unattended-reboot — nobody learns until the power cut.** The
*failure-is-silent* half is PROVEN unconditionally by the code above. The
*bind-fails* half is PROVEN for the isolated command shape.

### ⚠️ The Lenovo contradiction — unresolved, and I will not paper over it

The Lenovos reportedly **do** auto-unlock via Tang. That is direct evidence the
real installer path succeeds where my isolated repro failed. Both cannot be true
of the same code path, so one of these holds:

1. The runner **allocates a PTY** on that path, so `/dev/tty` opens and… someone
   answered the prompt? (Unlikely for an unattended install — it would hang.)
2. Tang was enrolled **by hand** after install, or from an interactive shell.
3. The Lenovos **also have no clevis token**, have never actually
   unattended-unlocked, and every observed boot had a human or a cached
   passphrase. **This would be the bigger finding.**

**One command decides it:**

```sh
cryptsetup luksDump /dev/nvme0n1p4 | grep -A2 -i token   # on len-serv-001
```

- **`clevis` token present** ⇒ the real path works; R2 downgrades to *"the
  config is fragile and must be hardened"*.
- **No `clevis` token** ⇒ R2 escalates: the fleet never had unattended unlock.

**I could not run it.** `jdfalk@172.16.3.92` connects, but `luksDump` needs root
and the sudo path was denied. **This is the first thing to check on Monday**, and
it is cheap. **COULD NOT VERIFY.**

**Either way, all three fixes below are correct and should land regardless** —
they convert a silent failure into a loud one, which is the actual defect.

### The fix also closes the `adv`-pinning hole — EXECUTED

The brief asks whether the missing `adv` is a trust-on-first-use hole. **It is,
at bind time** — and the same one-line change fixes both problems. Setting
`thp` takes the `elif` branch: no prompt, *and* the signing key is pinned.

| Test | Config | Result |
|---|---|---|
| **Fix** | `{url, thp}` ×3, **no `-y`**, non-TTY | **rc=0**, JWE 5499 B, round-trips |
| **Negative control** | one `thp` corrupted | **rc=1**, `Unable to fetch advertisement: 'http://172.16.2.45/adv/AAAA…'` |

The negative control proves pinning is *enforced*, not decorative. Real
thumbprints, read via clevis's own code path on 2026-07-17:

| Tang | `thp` (S256) |
|---|---|
| 172.16.2.45 | `R6vOh9w8zt0vtACXnTMVyLvRHKxenAeGejGmpW1WnU4` |
| 172.16.2.46 | `lp0iGP7Q58WJme-ZzudVRhJTyhBJpumXnxmMfboE-go` |
| 172.16.2.47 | `gHF961L4l5uB5FEPpqvLcHoyMBn8QMzgV2O76yLCR-Y` |

⚠️ Pinning converts *ongoing* TOFU into a **one-time** trust decision made at
the moment those values were read over plain HTTP on a trusted LAN. That is a
real, bounded improvement — not a proof of authenticity. Rotate a Tang key and
these must be re-pinned or bind fails closed (correct behaviour).

⚠️ Method note: my first hand-rolled `jose` pipeline returned a **wrong**
thumbprint for .47 (it read the `deriveKey` JWK, not the `verify` JWK) and an
empty one on a first pass. The table above uses clevis's own logic and is
cross-validated against the thumbprint clevis printed for .45. Do not re-derive
these with an ad-hoc pipeline.

## R3. The report misdiagnosed the dracut bug. The real one is a **zvol race**, and it is worse.

The report's gap #2 says the initramfs-tools/dracut asymmetry "hints at a real
bug" in **layout handling**. Both halves of that are wrong, and the actual bug
is more dangerous.

**Wrong half 1 — the zsys version is not layout-agnostic about depth.** The
report characterises it as layout-agnostic discovery. It is not:

```sh
for ks in $(find /dev/zvol/ -name 'keystore'); do
    pool="$(basename $(dirname ${ks}))"
```

`basename $(dirname …)` takes only the last component, so zsys **also** assumes
`<pool>/keystore`. Its genuine difference is **multi-pool** support, not depth
tolerance.

**Wrong half 2 — the dracut port is correct for the required layout.** With
`ENCRYPTIONROOT = rpool`, `ks=/dev/zvol/rpool/keystore` resolves and the
`file:///run/keystore/rpool/`* match fires. No layout bug. (And R1 proves that
layout is constructible.)

**The real bug: the dracut port dropped the wait loop.**

zsys (initramfs-tools) waits up to 5 s for zvol nodes to appear:

```sh
timeout=50
NUMKS=$(zfs list -H -o name | grep '/keystore$' | wc -l)
while [ ${NUMKS} -ne $(find /dev/zvol/ -name 'keystore' | wc -l) ]; do
    if [ $timeout -le 0 ]; then break; fi
    sleep .1
    timeout=$((timeout - 1))
done
```

The dracut port (`4001`) has **no wait, no `udevadm settle`, no retry**:

```sh
    ks="/dev/zvol/$pool/keystore"
    if [ ! -e "$ks" ]; then
        echo "Error: $ks does not exist." >&2
        return 1
    fi
```

Four independent facts make this a live race — all PROVEN:

1. `zvol_wait(1)`, `zfs-2.4.1`, verbatim: *"When a ZFS pool is imported, the
   volumes within it will appear as block devices. As they're registered,
   udev(7) **asynchronously** creates symlinks under `/dev/zvol`"*.
2. Upstream ships a dedicated unit for exactly this gap —
   `etc/systemd/system/zfs-volume-wait.service.in`, verbatim:
   `Description=Wait for ZFS Volume (zvol) links in /dev`, `After=zfs-import.target`,
   `ExecStart=@bindir@/zvol_wait`. **"pool imported" ≠ "zvol nodes exist"** is
   upstream's own position, encoded in a systemd unit.
3. `zfs-load-key.sh` waits only for `zfs-import.target`, then stats the zvol
   immediately:
   `while ! systemctl is-active --quiet zfs-import.target; do … sleep 0.1s; done`.
4. **`zvol_wait` is not in the initramfs.** `90zfs/module-setup.sh.in`'s
   essential binary list is verbatim
   `zgenhostid zfs zpool mount.zfs hostid grep awk tr cut head` — no `zvol_wait`.

And the port's *retry is on the wrong object*. Upstream's `file)` case does
have a ~10 s wait loop — but it waits for `${KEYFILE}`
(`/run/keystore/rpool/system.key`), which can never appear once the keystore
mount has already failed. The wait guards the symptom, not the cause, then:

```sh
[ -r "${KEYFILE}" ] || warn "ZFS: Key ${KEYFILE} for ${ENCRYPTIONROOT} hasn't appeared. Trying anyway."
zfs load-key "${ENCRYPTIONROOT}"
```

— which is verbatim the failure text quoted in the patch's own bug report.

**Grade: the race is PROVEN to exist structurally (async creation + zero
tolerance). Whether it fires on U1 is INFERRED and untested** — it is a timing
race, so it may pass a hundred times and fail on the boot that matters. It
lands on the unattended-reboot constraint. **The design must not rely on the
shipped patch alone (§D7).**

## R4. 🔴 The headline: a TPM2+PIN token on the keystore HANGS the boot forever.

The advisor asked whether a PIN prompt would stall the Tang path for
`token-timeout=30s`. The answer is worse: **it does not stall — it never
returns, and Tang is never reached.**

`src/cryptsetup/cryptsetup.c`, systemd **v259** (Ubuntu 26.04 ships
259.5-0ubuntu3), documents the ordering verbatim:

```c
                /* When we were able to acquire multiple keys, let's always process them in this order:
                 *
                 *    1. A key acquired via PKCS#11 or FIDO2 token, or TPM2 chip
                 *    2. The configured or discovered key, of which both are exclusive and optional
                 *    3. The empty password, in case arg_try_empty_password is set
                 *    4. We enquire the user for a password
                 */
```

**Tokens are tried first; the password path — the one clevis answers — is
last.** The token path runs whenever there is no keyfile:

```c
                /* Tokens are available in LUKS2 only, but it is ok to call (and fail) with LUKS1. */
                if (!key_file && use_token_plugins()) {
                        r = crypt_activate_by_token_pin_ask_password(
                                        cd, volume, /* type= */ NULL, until, /* userdata= */ NULL, flags,
                                        "Please enter LUKS2 token PIN:", "luks2-pin", "cryptsetup.luks2-pin");
```

And that helper, verbatim, ends in an **unbounded loop**:

```c
        if (FLAGS_SET(arg_ask_password_flags, ASK_PASSWORD_HEADLESS))
                return log_error_errno(SYNTHETIC_ERRNO(ENOPKG), "PIN querying disabled via 'headless' option. …");

        for (;;) {
                pins = strv_free_erase(pins);
                AskPasswordRequest req = { … .until = until, … };
                r = ask_password_auto(&req, flags, &pins);
                if (r < 0)
                        return r;
                …
        }
```

The deadline is nil. `verb_attach`:

```c
        until = usec_add(now(CLOCK_MONOTONIC), arg_timeout);
        if (until == USEC_INFINITY)
                until = 0;
```

with `static usec_t arg_timeout = USEC_INFINITY;` — and `arg_timeout` is only
settable through the crypttab **options** argument, which the Ubuntu patch does
not pass (§R5). **`until = 0` = no deadline.**

The chain, every link source-proven:

1. Keystore has a `systemd-tpm2` token with a PIN → `crypt_activate_by_token_pin`
   returns `-ENOANO` ("needs pin").
2. `$PIN` env var is absent in the initrd → no PIN from `acquire_pins_from_env_variable`.
3. `headless` is **unset** (unreachable, §R5) → the `ENOPKG` bail is skipped.
4. `for(;;)` + `ask_password_auto(until=0)` → **blocks forever** on
   `Id=cryptsetup.luks2-pin`.
5. `clevis-luks-askpass` matches only `Id=cryptsetup:*` (report §2, confirmed) —
   `cryptsetup.luks2-pin` does **not** match. **clevis will not answer it.**
6. The passphrase path (step 4 of the ordering comment) is **never reached**.

**⇒ `enroll_tpm2: true` + `tpm2_pin: <set>` — exactly what `unimatrixone.yaml`
asserts today — applied to the keystore, converts an unattended reboot into a
permanent hang at an unanswerable PIN prompt.**

The same reasoning applies to a `systemd-fido2` token
(`attach_luks2_by_fido2_via_plugin` → same helper → `"cryptsetup.fido2-pin"`),
which additionally cannot be satisfied at all without a physical touch.

**Grade: INFERRED (strong) — every link is code-proven at v259; not boot-proven.
This is the #1 VM-gate test (§D9).** It is also why §D3 enrolls **no
systemd-cryptenroll token on the keystore**.

## R5. 🔴 `headless=` — the report's prescribed fix would destroy Tang, and is unreachable anyway.

The report names `headless=` as the lever twice — §5 ("a fail-open-to-hang
behaviour we should set deliberately") and §8 ("`headless=` is the lever that
changes it"). Both are wrong, for two independent reasons.

**(a) It is unreachable in the recommended design.** `verb_attach`, verbatim:

```c
        /* Arguments: systemd-cryptsetup attach VOLUME SOURCE-DEVICE [KEY-FILE] [CONFIG] */
        assert(argc >= 3 && argc <= 5);
        const char *volume = ASSERT_PTR(argv[1]),
                *source = ASSERT_PTR(argv[2]),
                *key_file = argc >= 4 ? mangle_none(argv[3]) : NULL,
                *config = argc >= 5 ? mangle_none(argv[4]) : NULL;
        …
        if (config) {
                r = parse_crypt_config(config);
```

The Ubuntu patch invokes `systemd-cryptsetup attach "keystore-${pool}" "${ks}"`
— **argc = 3**. So `config = NULL`, `parse_crypt_config()` never runs, and
`headless=`, `token-timeout=`, and `tries=` are **all** unsettable. There is no
crypttab entry either (the report establishes this itself). The report
recommends pulling a lever its own architecture removes.

**(b) Worse — if it *were* reachable, it would break Tang.** `headless=` sets
`ASK_PASSWORD_HEADLESS`, and `get_password()` bails before ever posting an ask
file:

```c
        if (FLAGS_SET(arg_ask_password_flags, ASK_PASSWORD_HEADLESS))
                return log_error_errno(SYNTHETIC_ERRNO(ENOPKG), "Password querying disabled via 'headless' option.");
```

**clevis unlocks *by answering the interactive password prompt.* No ask file ⇒
no clevis ⇒ no Tang.** `headless=yes` on a clevis-backed volume disables the
only unattended path. This is a direct contradiction inside the report:
§7 correctly identifies that the design "runs on" the ask-password protocol,
while §5/§8 recommend switching that protocol off.

**⇒ `headless=` is not a lever we have, and not one we would want. Delete it
from the design vocabulary for any clevis volume.** PROVEN.

## R6. A silent-death invariant the report missed: clevis survives only because its token type is foreign.

`check_registered_passwords()` (v259), verbatim in relevant part:

```c
                type = sd_json_variant_string(w);
                if (STR_IN_SET(type, "systemd-recovery", "systemd-pkcs11", "systemd-fido2", "systemd-tpm2")) {
                        …
                        JSON_VARIANT_ARRAY_FOREACH(z, w) {
                                …
                                slots[u] = false;
                        }
                }
        …
        /* Check if any of the slots is not referenced by systemd tokens */
        for (int slot = 0; slot < slot_max; slot++)
                if (slots[slot]) {
                        passphrase_type |= PASSPHRASE_REGULAR;
                        break;
                }
```

and its caller:

```c
                                if (passphrase_type == PASSPHRASE_NONE) {
                                        passphrase_type = check_registered_passwords(cd);
                                        if (passphrase_type == PASSPHRASE_NONE)
                                                return log_error_errno(SYNTHETIC_ERRNO(EINVAL), "No passphrase or recovery key registered.");
                                }
```

If **every** active keyslot is referenced by a `systemd-*` token, systemd
returns `EINVAL` and **never posts an ask file** — clevis is never invoked,
Tang never runs. The design survives only because clevis's token type is
`"clevis"` (report §5, `clevis-luks-common-functions`), which is **not** in that
`STR_IN_SET`, so its keyslot stays "regular".

**⇒ Design invariant: the keystore header MUST always retain at least one
keyslot not referenced by a `systemd-*` token.** The report's §6 note that
"`luksKillSlot` reading the passphrase from stdin silently enables batch mode
and drops the last-keyslot guard" makes this a live operational landmine: a
slot cleanup that removes the clevis/passphrase slots would silently kill
unattended boot while leaving the box apparently fine until reboot. PROVEN.

## R7. §3(c) — the enrollment-surface argument. The report invited an attack; here it is.

The report calls this "in my judgement the strongest functional argument for
native encryption on this host". It has an **unstated premise**, and overstates
its cost model.

### (a) The unstated premise: dropping IMSM ≠ dropping mdadm

The report's table asserts post-IMSM ZFS-on-LUKS is forced to **2** LUKS
containers, "one per disk … both must unlock before import". That is only true
given an *additional* constraint the report never states: **that no md layer may
sit beneath LUKS.**

`mdadm` RAID1 (real Linux md, not IMSM fakeraid) across `sda3`+`sdb3` presents a
single `/dev/md*`, taking exactly **one** LUKS container — **1× enrollment,
real mirroring, no fakeraid, no IMSM.** The locked decision bans *IMSM
fakeraid* and prefers *native ZFS mirroring*; it does not make 2 containers a
technical necessity. As written, §3(c)'s headline ("Dropping IMSM makes
ZFS-on-LUKS strictly worse") is **false as stated**.

The report dismisses only a strawman — "one LUKS container on one disk holding
a keyfile that unlocks the second" — which is indeed bad, and is not the real
alternative.

**The correct rejection of md-RAID1-under-LUKS is a different argument
entirely, and the report should have made it:** with md beneath LUKS, ZFS sees
**one** vdev. ZFS can then detect corruption via checksum but **cannot repair
it**, because it has no second copy to repair *from* — md hands up a single
block and silently picks a half on mismatch. Native ZFS mirroring is what makes
scrub self-healing. **That** is the reason to choose native mirroring, and it
is about data integrity, not enrollment surface. (INFERRED from ZFS's
documented self-healing model; the report itself notes datasets "can be
scrubbed, resilvered … without the encryption keys being loaded".)

### (b) "2× everything" conflates scriptable and interactive cost

Two Tang binds and two TPM2 enrollments are a `for d in sda3 sdb3` loop — ~2×
one loop iteration, i.e. ≈ 0 marginal human effort. Only the genuinely
**interactive** factors double: a FIDO2 touch per container, a PIN typed per
container. The honest claim is "2× the interactive factors", not "2×
everything: 2 Tang bindings, 2 TPM2 enrollments, 2 FIDO2 slots, 2 rotations,
2 revocations".

### (c) The keystore introduces a failure mode two containers do not have

The report: *"a zvol inside a mirrored rpool is **mirrored by ZFS itself** …
The advisor-flagged objection … **does not apply to this design**."* This claims
too much.

The keystore is **one logical LUKS2 header**. ZFS mirrors its *blocks*
faithfully — including a **logically corrupt but checksum-valid write** (a torn
`luksAddKey`, a bad metadata update, an operator `luksKillSlot` mistake). Both
mirror halves receive the same corruption, because ZFS is doing its job. ZFS
protects against **device death and bit-rot**, not against **valid-but-wrong
writes**.

Two independent LUKS containers each carry an **independent header**: a botched
enrollment on disk A still leaves B openable. So the keystore is **not "strictly
better"** — it trades an *independent-header* failure domain for a
*device-death* one. Different surface, not a dominant one.

**Verdict on §3(c): the conclusion (native + keystore) survives, but not on the
reasoning given.** The real decisive arguments are (i) ZFS self-healing requires
native mirroring, and (ii) Ubuntu ships and maintains the keystore path. The
enrollment-surface framing should be retired. *Mitigation for (c) is in §D5:
back up the LUKS header — which then collides with the report's own §6
revocation finding, an accepted, documented tension.*

## R8. Smaller corrections

| # | Report claim | Finding |
|---|---|---|
| 1 | §7: patch applied at `debian/patches/series:15` | **Confirmed.** Cloned `ubuntu/resolute`; it is line 15. Patch body matches the report's excerpt verbatim. |
| 2 | §3: "The keystore is a zvol inside rpool … inherits ZFS mirroring for free" | True, but see R7(c) — mirroring ≠ independence. |
| 3 | §8/§4: TPM2+PIN is a viable break-glass rung | **Refuted for the keystore design** (R4): it is not a fallback, it is a boot-hang. |
| 4 | §5: "`token-timeout=` defaults to 30s, after which authentication via password is attempted — the automatic degradation path" | `arg_token_timeout_usec = 30*USEC_PER_SEC` is real, but **unsettable here** (R5a), and it does **not** bound the token-plugin PIN loop, which uses `until` (= 0). The "automatic degradation path" does not exist in this design. |
| 5 | Gap #2 "does not change the architecture choice, but it changes the exact `zfs create` commands" | Correct — and now resolved (R1, §D2). |
| 6 | §1: "there is no true key rotation for ZFS native encryption" | Accepted and load-bearing — see §D5/§D8. |

**Where the report is right and deserves credit:** the LUKS2-only findings
(Finding 1), the `keylocation` scheme table (Finding 2), the identification of
Ubuntu's keystore patch as the distro path (Finding 3), the FIDO2 user-presence
proof (§6), the `hostonly`/`hostonly_cmdline` analysis and the resulting
"Tang gets no network automatically" gap (§7), and the `luksKillSlot`-is-not-
revocation finding (§6) all held up under checking. The §7 chain is correct as
far as it goes; it is **incomplete** (R3, R4), not wrong.

---

# PART 2 — THE DESIGN

## D1. Partition layout

Two identical 931 GB SATA disks (`sda`, `sdb`), addressed **by
`/dev/disk/by-id/`**, never by kernel name (the `md126/md127` swap recorded in
`unimatrixone.yaml` is precisely this hazard).

Per disk, identically:

| # | Size | Type | Purpose |
|---|---|---|---|
| p1 | 1 GiB | `EF00` | **ESP** — independent per disk, FAT32 |
| p2 | 4 GiB | `8300` | RESET (ext4) — preserved from the current scheme |
| p3 | 2 GiB | `BE00` | **bpool** member — unencrypted ZFS mirror |
| p4 | rest (~924 GiB) | `BF00` | **rpool** member — natively-encrypted ZFS mirror |

```
sda ─┬─ sda1  ESP #1      (FAT32, independent)
     ├─ sda2  RESET
     ├─ sda3  bpool  ──┐
     └─ sda4  rpool  ─┐│
sdb ─┬─ sdb1  ESP #2   ││ (FAT32, independent)
     ├─ sdb2  RESET    ││
     ├─ sdb3  bpool  ──┼┴─ bpool  = mirror(sda3, sdb3)   UNENCRYPTED (GRUB reads it)
     └─ sdb4  rpool  ──┴── rpool  = mirror(sda4, sdb4)   ZFS-native encrypted
nvme0n1 (Optane 13.4G) ── UNUSED (see D1.3)
```

### D1.1 ESPs: two independent, **not** mdadm RAID1

The locked constraint permits mdadm on the ESP. **I recommend against it**,
and this is a considered rejection, not an omission:

- RAID1 on an ESP only works with `metadata=1.0` (superblock at the **end**) so
  firmware sees a plain FAT32. That is the whole trick, and it is load-bearing.
- But the firmware, `efibootmgr`, and `fwupd` all write to the ESP **behind
  md's back**, through the raw partition. md does not observe those writes, so
  the halves silently diverge, and a later resync can propagate the **stale**
  half over the fresh one. This is a known, well-documented hazard of the
  pattern.
- The upside — a single mount point — is worth very little here; the downside is
  a bootloader corrupted by the mechanism meant to protect it, on the host whose
  binding constraint is unattended reboot.

**Instead:** install GRUB to **both** ESPs, and sync on every bootloader/kernel
change (§D7.3). Each disk is then **independently bootable** — pull either disk
and the box still boots, which is exactly the property a mirrored ESP is
supposed to buy, obtained without the divergence hazard.

Grade: **INFERRED.** The firmware-writes-behind-md mechanism is well established
but I did not execute a divergence test.

### D1.2 Swap: zram, no disk swap

Swap on a ZFS zvol is a long-standing deadlock hazard under memory pressure
(ZFS needs to allocate to write out, writing out needs the allocation).
**Recommend `zram` only, no disk swap partition.** This also removes any
question of plaintext leaking to an unencrypted swap area. If disk swap is
later required, it must be a **per-disk partition with a random ephemeral key**
(never a zvol, never a shared keystore slot).
Grade: **INFERRED** (the zvol-swap deadlock is widely reported upstream; not
re-verified here).

### D1.3 The Optane (13.4 GB): **unused** — argued, not defaulted

I evaluated four roles and reject all of them:

| Role | Verdict |
|---|---|
| **bpool / ESP** | ❌ Boot-critical on a **single unmirrored device**. Reintroduces exactly the SPOF that dropping IMSM for native mirroring exists to remove, and contradicts the binding unattended-reboot constraint. |
| **keystore** | ❌ Same SPOF, and strictly worse than the current design: the keystore-in-rpool gets ZFS mirroring free; on Optane it does not. Optane death ⇒ unbootable. |
| **SLOG** | ❌ 13.4 GB is far more than a SLOG needs, so the capacity is wasted anyway; an **unmirrored** SLOG adds a device whose failure semantics are subtle for zero benefit on a root pool with no sync-write workload. |
| **L2ARC** | ⚠️ Defensible — L2ARC loss is harmless by design and is not boot-critical. But 13.4 GB against a ~924 GiB pool is marginal, it consumes ARC headers (RAM) to index, and L2ARC/native-encryption interaction is a detail I did **not** verify. |

**Decision: leave the Optane unused and unpartitioned.** L2ARC is the only
non-harmful option and its benefit is unproven; adding it buys little and adds a
variable to a host that already has an unresolved boot history
(`project_supermicro_unimatrixone_boot_hang`). **"Unused" is the honest answer:
the binding constraint is availability, and every role that would use this
device either degrades availability or is speculative.** Revisit only after U1
boots reliably, and only as `cache`.

## D2. Key hierarchy

```
                     ┌─────────────────────────────────────────┐
                     │  ZFS MASTER KEY (per-dataset, immortal) │  ← never rotatable
                     │  lives: rpool metadata; kernel RAM when │    (zfs change-key
                     │         loaded                          │     does NOT rotate it)
                     └────────────────▲────────────────────────┘
                                      │ wrapped by
                     ┌────────────────┴────────────────────────┐
                     │  ZFS WRAPPING KEY  = system.key         │
                     │  32 random bytes, keyformat=raw         │
                     │  encryptionroot = rpool  (BARE POOL)    │
                     │  keylocation = file:///run/keystore/rpool/system.key
                     │  at rest: ONLY inside the keystore fs   │
                     └────────────────▲────────────────────────┘
                                      │ stored on
                     ┌────────────────┴────────────────────────┐
                     │  ext4 on /dev/mapper/keystore-rpool     │
                     │  mounted at /run/keystore/rpool (initrd)│
                     └────────────────▲────────────────────────┘
                                      │ unlocked by
                     ┌────────────────┴────────────────────────┐
                     │  LUKS2 on /dev/zvol/rpool/keystore      │
                     │  (zvol, encryption=off, 128 MiB)        │
                     │  ── LUKS2 VOLUME KEY ──                 │
                     └──▲──────────────▲──────────────▲────────┘
                        │              │              │
         ┌──────────────┴──┐  ┌────────┴───────┐  ┌───┴──────────────┐
         │ clevis token    │  │ passphrase     │  │ systemd-recovery │
         │ sss 2-of-3 Tang │  │ keyslot        │  │ keyslot          │
         │ thp-PINNED      │  │ (install key)  │  │ (printed once)   │
         │ ✅ UNATTENDED   │  │ ❌ human@SOL   │  │ ❌ human@SOL     │
         └─────────────────┘  └────────────────┘  └──────────────────┘

         ❌ NO systemd-tpm2 token   ┐ deliberately absent — see R4:
         ❌ NO systemd-fido2 token  ┘ either one HANGS the unattended boot
```

**Why `rpool` (the bare pool root) is the encryption root** — forced by the
shipped patch, and now proven constructible (R1):

- `zfs-load-key.sh` computes `ENCRYPTIONROOT="$(zfs get -Ho value encryptionroot "${dataset}")"`
  for `BOOTFS = rpool/ROOT/ubuntu` → **`rpool`**.
- The patch matches `"file:///run/keystore/${ENCRYPTIONROOT}/"*` → requires
  `keylocation = file:///run/keystore/rpool/system.key`. ✅
- `_open_and_mount_luks_keystore rpool …` → `ks=/dev/zvol/rpool/keystore`. ✅
- `[ "$(zpool get -Ho value feature@encryption "${BOOTFS%%/*}")" = 'active' ]`
  → `rpool` → `active` because the root dataset is encrypted. ✅
- `rpool/keystore` with `encryption=off` is legal (R1) and readable **before**
  `zfs load-key`, because its blocks are plaintext. **No circularity.**

### Exact commands

Install-time (live environment). `${SDA}`/`${SDB}` are `/dev/disk/by-id/…` paths.

```sh
# ---- 1. bpool: unencrypted, GRUB-readable feature set ----
zpool create -f -o ashift=12 -o autotrim=on \
    -o compatibility=grub2 \
    -O compression=lz4 -O acltype=posixacl -O xattr=sa \
    -O devices=off -O normalization=formD -O relatime=on \
    -O canmount=off -O mountpoint=none -R /mnt/targetos \
    bpool mirror ${SDA}-part3 ${SDB}-part3

# ---- 2. wrapping key on tmpfs (temporary location) ----
install -m 0600 /dev/null /run/uaa-system.key
dd if=/dev/urandom of=/run/uaa-system.key bs=32 count=1 status=none   # 32 bytes, raw

# ---- 3. rpool: BARE POOL is the encryption root ----
zpool create -f -o ashift=12 -o autotrim=on \
    -O compression=lz4 -O acltype=posixacl -O xattr=sa -O dnodesize=auto \
    -O normalization=formD -O relatime=on \
    -O canmount=off -O mountpoint=none -R /mnt/targetos \
    -O encryption=aes-256-gcm \
    -O keyformat=raw \
    -O keylocation=file:///run/uaa-system.key \
    rpool mirror ${SDA}-part4 ${SDB}-part4

# ---- 4. the keystore zvol: UNENCRYPTED child of an encrypted parent (R1) ----
zfs create -V 128M -b 4096 \
    -o encryption=off \
    -o compression=off \
    -o sync=always \
    -o com.sun:auto-snapshot=false \
    rpool/keystore
udevadm settle
zvol_wait || true          # bounded; node must exist before luksFormat

# ---- 5. LUKS2 on the zvol ----
cryptsetup luksFormat --batch-mode --type luks2 \
    --key-file /run/.uaa-luks-setup.key /dev/zvol/rpool/keystore
cryptsetup open --key-file /run/.uaa-luks-setup.key \
    /dev/zvol/rpool/keystore keystore-rpool

mkfs.ext4 -F -L keystore /dev/mapper/keystore-rpool
mkdir -p /run/keystore/rpool
mount -o discard /dev/mapper/keystore-rpool /run/keystore/rpool

# ---- 6. move the wrapping key INTO the keystore, then re-point keylocation ----
install -m 0400 /run/uaa-system.key /run/keystore/rpool/system.key
zfs set keylocation=file:///run/keystore/rpool/system.key rpool
shred -u /run/uaa-system.key

# ---- 7. datasets (all inherit encryption from rpool) ----
zfs create -o canmount=off -o mountpoint=none rpool/ROOT
zfs create -o canmount=noauto -o mountpoint=/ rpool/ROOT/ubuntu
zfs mount rpool/ROOT/ubuntu
zfs create -o canmount=off -o mountpoint=none bpool/BOOT
zfs create -o mountpoint=/boot bpool/BOOT/ubuntu
zpool set bootfs=rpool/ROOT/ubuntu rpool
```

⚠️ `-o compatibility=grub2` on bpool supersedes the older hand-listed
`-d -o feature@…` incantation. **COULD NOT VERIFY** that resolute's zfs-linux
ships the `grub2` compatibility file (`/usr/share/zfs/compatibility.d/grub2`);
if absent, fall back to the explicit feature list. Ubuntu additionally ships
`4100-disable-bpool-upgrade.patch`, which protects bpool from feature drift
(report §1) — that is in our favour and should not be circumvented.

⚠️ `sync=always` on the keystore zvol is deliberate: LUKS header writes
(enrollment, slot changes) must not be lost to a power cut mid-enrollment. This
partially mitigates R7(c). Cost is negligible — the keystore is written a
handful of times in its life.

**Resulting property table (what to assert in `verify`):**

| Dataset | `encryption` | `encryptionroot` | `keystatus` pre-`load-key` |
|---|---|---|---|
| `rpool` | `aes-256-gcm` | `rpool` | `unavailable` |
| `rpool/keystore` | **`off`** | **`-`** | n/a |
| `rpool/ROOT` | `aes-256-gcm` | `rpool` | `unavailable` |
| `rpool/ROOT/ubuntu` | `aes-256-gcm` | `rpool` | `unavailable` |
| `bpool/BOOT/ubuntu` | `off` | `-` | n/a |

## D3. Enrollment paths

All enrollment targets **`/dev/zvol/rpool/keystore`** — not a partition. This
is a real change from today's code, which binds `partition_path(disk, 4)`
directly: the target now **does not exist until after `zpool create`**, so
enrollment must move *after* pool creation in the install sequence.

### D3.1 Tang — clevis SSS 2-of-3, thumbprint-pinned ✅ THE ONLY UNATTENDED PATH

**When:** install time, immediately after the keystore LUKS container is opened.

```sh
clevis luks bind -d /dev/zvol/rpool/keystore -k /run/.uaa-luks-setup.key sss '{
  "t": 2,
  "pins": {
    "tang": [
      {"url":"http://172.16.2.45","thp":"R6vOh9w8zt0vtACXnTMVyLvRHKxenAeGejGmpW1WnU4"},
      {"url":"http://172.16.2.46","thp":"lp0iGP7Q58WJme-ZzudVRhJTyhBJpumXnxmMfboE-go"},
      {"url":"http://172.16.2.47","thp":"gHF961L4l5uB5FEPpqvLcHoyMBn8QMzgV2O76yLCR-Y"}
    ]
  }
}'
```

**Binds to:** 2-of-3 Tang servers answering on the LAN. Tolerates one down.
**Unattended reboot: YES** — the only method that qualifies.

**Three mandatory changes to the current code:**

1. **`thp` is REQUIRED, not optional.** Without it the bind fails on a non-TTY
   channel (R2, EXECUTED). It also closes the bind-time TOFU hole. Add a
   `thp: String` field to `TangServer` in `config.rs` and make it
   **non-optional** so a config without it fails to parse — fail-closed.
2. **The bind MUST be FATAL.** Replace the `warn!(… non-fatal …)` in
   `enroll_tang_clevis` with a hard error. A Tang bind that silently no-ops
   produces a host that cannot meet its binding constraint, and hides it until
   the outage. This is the single highest-value line change in the repo.
3. **`verify` must assert a `clevis` token exists** on the keystore
   (`cryptsetup luksDump /dev/zvol/rpool/keystore | grep clevis`), not merely
   that the config *asked* for one.

*(`-y` would also make the bind succeed, but it accepts whatever key the
network offers at bind time. Pinning is strictly better and costs one field.
Do **not** use `-y`.)*

### D3.2 Recovery key — the real backstop ❌ not unattended

**When:** install time, after the Tang bind.

```sh
PASSWORD="$(cat /run/.uaa-luks-setup.key)" \
  systemd-cryptenroll --recovery-key /dev/zvol/rpool/keystore
```

**Binds to:** nothing — a high-entropy computer-generated key, printed **once**,
to be stored in the password manager out-of-band.
**Unattended reboot: NO** (human at SOL). **This is the rung that must exist**
(report §8 is right about this), because after R4 it is now the *only*
remote break-glass.

⚠️ `systemd-recovery` is a `systemd-*` token type, so it is counted by
`check_registered_passwords` (R6). It is safe **only because** the clevis
passphrase keyslot remains unreferenced by any systemd token. Never remove the
clevis slot.

⚠️ There is **no** `libcryptsetup-token-systemd-recovery.so` plugin — recovery
keys are entered as passwords — so a recovery key does **not** trigger the R4
token-plugin hang. This is the reason recovery, and not TPM2+PIN, is the
break-glass. **INFERRED** (no recovery plugin found in systemd's
`src/cryptsetup/cryptsetup-tokens/`); **VM-gate it** (§D9).

### D3.3 TPM2 + PIN — ❌ **DO NOT ENROLL ON THE KEYSTORE**

**Recommendation: remove `enroll_tpm2: true` / `tpm2_pin` from
`unimatrixone.yaml`.** Four independent reasons, in descending force:

1. **It hangs the unattended boot forever** (R4). Decisive on its own.
2. **U1 may have no TPM at all.** The report's gap #1: JTPM1 ships **empty**,
   and nothing in the repo records a module being purchased. `enroll_tpm2: true`
   may be asserting the impossible.
3. **The PCR policy would be brittle.** No UKI ⇒ no PCR 11 signed policy
   (report §5); literal PCRs on a host with a **CMOS-clear history**
   (`project_supermicro_unimatrixone_boot_hang`) is a foot-gun.
4. **It buys nothing Tang does not already give**, and a PIN-less variant would
   *weaken* the theft story (§D8.2).

If TPM2 is ever wanted here, it needs a different injection point than the
keystore attach — not a config flag.

### D3.4 FIDO2 / YubiKey — ❌ **DO NOT ENROLL ON THE KEYSTORE**

**Recommendation: set `expect_fido2: false` for U1.**

- A `systemd-fido2` token on the keystore hits the **same R4 hang**
  (`attach_luks2_by_fido2_via_plugin` → same helper → `"cryptsetup.fido2-pin"`),
  and additionally cannot be satisfied without a physical touch — so the box
  hangs until someone drives to it.
- FIDO2 cannot be unattended at the spec level anyway (report §6, PROVEN) and
  **a touch cannot traverse IPMI SOL** — so it was never going to serve U1's
  actual failure mode (remote operator, 3am).
- The report already concedes FIDO2 must be downgraded to on-site break-glass;
  R4 upgrades that from "downgrade" to "actively harmful **on this volume**".

`expect_fido2: true` is currently an *asserted requirement* in
`unimatrixone.yaml` and drives `verify`. It must be flipped, or `verify` will
demand a token whose presence would brick the host.

*(FIDO2 remains fine on the **Lenovos**, whose constraint set differs. This is a
U1-specific decision.)*

### D3.5 The install passphrase keyslot

Slot 0, from `config.luks_key`, created by `luksFormat`. Retained as the
plain-passphrase rung and — per R6 — as the guarantee that
`check_registered_passwords` returns `PASSPHRASE_REGULAR` so the ask file is
posted at all. **Never wipe it.**

## D4. Boot sequence

Power-on → mounted root. Injection points marked.

```
 1. UEFI firmware ──► ESP #1 (sda1)  [ESP #2 is the manual fallback, D7.3]
 2. shimx64.efi ──► grubx64.efi                      (Secure Boot chain intact)
 3. GRUB reads bpool  ── UNENCRYPTED, no key needed  ◄── why bpool exists
 4. GRUB loads vmlinuz + initrd
        cmdline: root=zfs:rpool/ROOT/ubuntu rd.neednet=1 ip=dhcp
                 console=ttyS0,115200 (SOL)   [NO rd.zfs.force — does not exist]
 5. dracut-cmdline.service
 6. dracut-pre-udev.service ──► systemd-udevd
 7. dracut-pre-trigger ──► systemd-udev-trigger
        └─ NIC driver loaded  ◄── forced via add_drivers (hostonly would omit it)
 8. dracut-initqueue.service
        └─ network up (rd.neednet=1 ip=dhcp)  ──► network-online.target
 9. sysinit.target
        ├─ cryptsetup.target  ◄── reached ONLY because 71systemd-cryptsetup
        │                         installs sysinit.target.wants/cryptsetup.target
        │                         (pulled in by zfs-dracut's depends(), the 4001 patch)
        └─ clevis-luks-askpass.path ARMS here
                 (DirectoryNotEmpty=/run/systemd/ask-password)
                 ◄── ★ our drop-in adds After=network-online.target (D7.2)
10. zfs-import.target       (pool imported — NO key needed: pool structure is plaintext)
        └─ udev BEGINS creating /dev/zvol/* symlinks  ── ASYNCHRONOUSLY ──┐
11. dracut-pre-mount.service   (After=basic.target dracut-initqueue.service cryptsetup.target)
        │
        ├─ ★★ pre-mount/89: 91uaa-keystore-wait  ◄── WE SHIP THIS (D7.1)
        │       bounded poll for /dev/zvol/rpool/keystore  ── CLOSES THE R3 RACE ──┘
        │
        └─ pre-mount/90: zfs-load-key.sh
             ├─ spin-wait: systemctl is-active zfs-import.target
             ├─ ENCRYPTIONROOT=$(zfs get encryptionroot rpool/ROOT/ubuntu) → "rpool"
             ├─ KEYLOCATION → file:///run/keystore/rpool/system.key  → MATCHES
             ├─ _open_and_mount_luks_keystore rpool /run/keystore/rpool/system.key
             │     ├─ ks=/dev/zvol/rpool/keystore                    [exists — D7.1 waited]
             │     ├─ systemd-cryptsetup attach keystore-rpool $ks   [argc=3: no keyfile, no config]
             │     │     ├─ token plugins tried FIRST
             │     │     │     └─ ★ NO systemd-tpm2/fido2 token present  ◄── D3.3/D3.4
             │     │     │        ⇒ nothing to hang on (R4 avoided BY CONSTRUCTION)
             │     │     ├─ check_registered_passwords() → PASSPHRASE_REGULAR
             │     │     │     ◄── holds because the clevis slot is not systemd-referenced (R6)
             │     │     └─ get_password() ──► posts /run/systemd/ask-password/ask.XXXX
             │     │              Id=cryptsetup:/dev/zvol/rpool/keystore
             │     │                        │
             │     │                        ▼
             │     │            ★ clevis-luks-askpass.path FIRES
             │     │              └─ clevis-luks-askpass reads the clevis token off the
             │     │                 LUKS2 header (NOT /etc/crypttab — there is no entry)
             │     │                 └─ sss 2-of-3 ──► curl Tang 172.16.2.45/46/47
             │     │                    └─ answers the socket with the passphrase  ✅ UNATTENDED
             │     ├─ /dev/mapper/keystore-rpool appears
             │     └─ mount -o discard → /run/keystore/rpool
             └─ zfs load-key rpool   (reads file:///run/keystore/rpool/system.key)
12. sysroot.mount ──► rpool/ROOT/ubuntu
13. dracut-pre-pivot ──► switch-root ──► systemd (real root)
14. /boot from bpool/BOOT/ubuntu; ESP #1 at /boot/efi
```

**Where each method injects:**

| Method | Injection point | Unattended |
|---|---|---|
| **Tang** | step 11, `clevis-luks-askpass` answers `Id=cryptsetup:*` | ✅ **YES** |
| Install passphrase | step 11, same ask file, typed at SOL | ❌ |
| Recovery key | step 11, same ask file, typed at SOL | ❌ |
| ~~TPM2+PIN~~ | ~~before the ask file~~ | ❌ **hangs — not enrolled** |
| ~~FIDO2~~ | ~~before the ask file~~ | ❌ **hangs — not enrolled** |

## D5. Failure modes

| # | What fails | What happens | Open/Closed | Recovery |
|---|---|---|---|---|
| 1 | **1 Tang down** | sss 2-of-3 still satisfied; boots normally | ✅ closed, **available** | None needed. Fix Tang at leisure. |
| 2 | **2+ Tang down** (quorum lost) | ask file posted, clevis cannot answer, **`until=0` ⇒ blocks forever** at the SOL prompt | 🔒 **closed, HUNG** | **§D6(b)**: type recovery key at SOL. Box waits indefinitely — it does **not** give up, and does **not** degrade. |
| 3 | **Network down / NIC driver missing** | Same as #2 — indistinguishable from Tang down | 🔒 closed, hung | Same. See D8.1 — this is the attacker-triggerable case. |
| 4 | **zvol node late** (R3 race) | *With D7.1:* waits, proceeds. *Without:* `Error: /dev/zvol/rpool/keystore does not exist.` → `zfs load-key` fails → **emergency shell** | 🔒 closed | D7.1 is mandatory. |
| 5 | **One disk dies** | rpool + bpool import DEGRADED; keystore zvol still readable from survivor; boots **if firmware picks the surviving disk's ESP** | ✅ closed, available | Replace disk, `zpool replace`, re-run D7.3 ESP sync. |
| 6 | **Dead disk was ESP #1** | Firmware falls through to ESP #2 **only if** BootOrder has it | ⚠️ **available only if pre-staged** | D7.3 must register **both** ESPs in NVRAM at install. Otherwise: manual boot-menu selection at SOL. |
| 7 | **Keystore LUKS header logically corrupted** (R7c) | Both mirror halves carry it — ZFS cannot help | 🔒 closed, **DATA LOSS** | `cryptsetup luksHeaderRestore` from the install-time backup. **Without that backup this is unrecoverable.** |
| 8 | **`system.key` lost** (keystore fs corrupt) | rpool master key unwrappable | 🔒 closed, **TOTAL DATA LOSS** | None. Restore from backup. See D5.1. |
| 9 | **Tang key rotated server-side** | `thp` no longer matches → clevis cannot derive → same as #2 | 🔒 closed, hung | Re-bind with new `thp`. **Fail-closed is correct here** — it is the pinning working. |
| 10 | **Clevis slot wiped during cleanup** (R6) | `check_registered_passwords` → `PASSPHRASE_NONE` → **ask file never posted** → Tang silently dead | 🔒 closed, hung, **silent until reboot** | Re-bind. Prevent via D7.4 guard. |
| 11 | **TPM2/FIDO2 token added later** by a well-meaning operator | R4 hang on next unattended boot | 🔒 closed, hung | Remove the token. **Prevent via D7.4.** |
| 12 | **`zfs change-key` run** | Wrapping key rotated; **master key unchanged and old wrapped key remains on disk** (report §1, PROVEN) | ⚠️ **security no-op** | Not a rotation. See D8.5. |

### D5.1 The consequence nobody should discover at 3am

**The keystore is a single logical object whose loss destroys the pool.** ZFS
mirroring protects it from device death (#5) but not from logical corruption
(#7) or fs corruption (#8). Therefore, **mandatory at install time**:

```sh
cryptsetup luksHeaderBackup /dev/zvol/rpool/keystore \
    --header-backup-file /run/keystore-rpool-header.img
# ── transport OFF-HOST, store in the password manager alongside the recovery key
```

⚠️ **This directly collides with the report's §6 finding** — a header backup
plus a passphrase valid at backup time defeats `luksKillSlot` revocation
**forever** (`cryptsetup-luksHeaderBackup(8)`, quoted verbatim in the report).
**Both facts are true and the tension is irreducible:** without the backup,
failure #7 is unrecoverable data loss; with it, revocation is not real.

**Decision: take the backup.** Availability is the binding constraint, and the
backup's threat model (an attacker who obtains the off-host backup *and* a
then-valid passphrase) is strictly narrower than the failure it prevents.
**Consequence to accept and write down: revocation on this host means
`cryptsetup reencrypt` + destroying every header backup — not `luksKillSlot`.**
This confirms the report's warning that `luks_keys.rs` /
`TASK-02-luks-rotate-revoke-guard` treating slot-kill as revocation is unsound,
and it is now load-bearing here.

## D6. Break-glass procedure — literal runbook (remote, IPMI SOL only)

Assumes: operator is **remote**, U1 is **headless**, the only channel is **IPMI
SOL at 172.16.3.150** (ADMIN/ADMIN). No hands on the box, no keyboard, no
YubiKey touch possible.

### D6.0 Getting a console — do this first, every time

> ⚠️ **`ipmitool` must be run FROM THE SERVER (172.16.2.30), NOT from the Mac.**
> macOS `ipmitool` crashes against Supermicro BMCs
> (`reference_ipmitool_from_server`). This is not optional trivia — it is step 0.

```sh
ssh 172.16.2.30
ipmitool -I lanplus -H 172.16.3.150 -U ADMIN -P ADMIN sol activate
#   ~.   to exit SOL      |    deactivate first if the session is stuck:
ipmitool -I lanplus -H 172.16.3.150 -U ADMIN -P ADMIN sol deactivate
```

> ⚠️ **The prompt must actually REACH the serial console.** D4 puts
> `console=ttyS0,115200` on the cmdline, but plymouth can still swallow the
> password prompt — this is exactly [clevis#248](https://github.com/latchset/clevis/issues/248),
> whose workaround is *"disable the splash screen (plymouth)"*. **Therefore:
> `GRUB_CMDLINE_LINUX` must NOT contain `splash`, and should carry `nosplash`.**
> If you reach SOL and see no prompt but the box is clearly hung, this is the
> first suspect: reboot, interrupt GRUB at the menu (`e`), strip `splash`, and
> boot once to get the prompt.

### D6(a) — 1 Tang server down

**Symptom:** none. The box boots normally.
**Steps:** *none.* sss 2-of-3 is satisfied by the surviving two.
**Action:** repair the RPi at leisure. **Do not** treat as an incident.
**Verify afterwards:** `clevis luks list -d /dev/zvol/rpool/keystore`.

### D6(b) — 2+ Tang down / quorum lost ← **the likely real event**

**Symptom:** U1 does not come up. On SOL you see a ZFS/cryptsetup password
prompt, hung. It will wait **forever** (`until = 0`, R4/R5) — it does **not**
time out, retry, or degrade.

```
1. ssh 172.16.2.30
2. ipmitool -I lanplus -H 172.16.3.150 -U ADMIN -P ADMIN sol activate
3. Confirm you see:  "Please enter passphrase for disk keystore-rpool..."
      └─ no prompt but box hung?  → plymouth (D6.0) or the prompt is on tty0 only.
4. Type the RECOVERY KEY (from the password manager, printed once at install
   per D3.2). NOT the install passphrase — either works, recovery key is the
   one that is meant to be stored.
5. Boot proceeds: keystore opens → system.key read → zfs load-key → root mounts.
6. Exit SOL with  ~.
7. THEN fix Tang. Until >=2 respond, EVERY reboot needs a human. Treat U1 as
   pinned until quorum returns.
```

**Do not** attempt to "fix" this by adding a TPM2/FIDO2 token as a fallback —
that makes the *next* unattended boot hang **before** the prompt you just used
(R4). The recovery key is the fallback.

### D6(c) — TPM cleared / PCRs changed

**N/A by design — and deliberately so.** D3.3 enrolls **no** TPM2 token on the
keystore, because a `systemd-tpm2` PIN token would hang every unattended boot
(R4). U1 may also have no TPM module at all (report gap #1).

- A CMOS clear, firmware update, or PCR change therefore **cannot** affect U1's
  unlock path. This is a *feature* of the design, bought deliberately: the host
  with a known CMOS-clear history (`project_supermicro_unimatrixone_boot_hang`)
  has **zero** PCR-bound secrets.
- **If someone later enrolls TPM2 anyway:** a cleared TPM/changed PCR then means
  the token fails → `-ENOANO` → `for(;;)` PIN loop → **permanent hang, and the
  recovery-key prompt in D6(b) is never reached.** Recovery = boot rescue media
  and `systemd-cryptenroll --wipe-slot=tpm2` on the keystore zvol (which
  requires importing rpool first — awkward and slow). **This is why D3.3 says
  don't.**

### D6(d) — YubiKey lost

**N/A by design.** D3.4 enrolls **no** FIDO2 token on the keystore. Losing a
YubiKey has **no effect** on U1's unlock path.

- FIDO2 could never have served this host's real failure mode anyway: **a touch
  cannot traverse IPMI SOL** (report §6, PROVEN), and the operator is remote.
- ⚠️ `expect_fido2: true` in `unimatrixone.yaml` must be flipped to `false`, or
  `verify` will demand a token whose presence would brick the host (D3.4).
- *(On the Lenovos, FIDO2 remains enrolled and the usual multi-key/revocation
  story applies — report §6. This section is U1-only.)*

### D6(e) — TOTAL: no Tang, no TPM, no recovery key

The ladder, in order. Each rung assumes the one above failed.

```
1. INSTALL PASSPHRASE.  The luksFormat slot-0 passphrase (config.luks_key,
   injected at place-time) is still enrolled and still works at the same SOL
   prompt as D6(b). Check the password manager / the place-time secret store
   BEFORE concluding the key is gone. Most "total" events end here.

2. LUKS HEADER BACKUP.  If the keystore's LUKS2 header is corrupt (failure #7)
   but you still hold a passphrase valid AT BACKUP TIME:
     - boot rescue media (netboot/USB), import the pool WITHOUT the key:
         zpool import -N -f rpool          # works: pool structure is plaintext
         udevadm settle; zvol_wait
     - restore the header onto the zvol:
         cryptsetup luksHeaderRestore /dev/zvol/rpool/keystore \
             --header-backup-file keystore-rpool-header.img
     - then unlock normally and reboot.
   (The backup is mandated by D5.1 and lives off-host with the recovery key.)

3. NOTHING LEFT.  If the recovery key, the install passphrase, AND the header
   backup are all gone:
     >>> THE DATA IS UNRECOVERABLE. <<<
   Not "hard" — unrecoverable. The ZFS master key is wrapped by system.key;
   system.key lives ONLY inside the keystore; the keystore opens ONLY with a
   LUKS2 keyslot. No keyslot, no key, no data. Tang cannot help (it holds no
   secret — it only participates in a derivation whose JWE lives in the header
   you just lost).
   ACTION: rebuild U1 from scratch and restore from backup. There is no
   forensic path, no vendor escrow, no support call.
```

> **The one-line consequence to internalise:** U1's data survives exactly as
> long as *one of* {2-of-3 Tang, recovery key, install passphrase, header
> backup} survives. Three of those four live **off the host**. Losing all four
> is a total loss with no remedy — so **verify at install time that the
> recovery key and the header backup actually left the machine and landed in
> the password manager.** That verification is the single cheapest insurance in
> this entire design.

---

## D7. Required implementation changes

### D7.1 ★ Ship `91uaa-keystore-wait` — MANDATORY (closes R3)

A minimal dracut module. **Additive: it waits only, then lets the shipped patch
do the attach+mount at priority 90.** No duplication, no fighting the distro.

`/usr/lib/dracut/modules.d/91uaa-keystore-wait/module-setup.sh`:

```sh
#!/bin/bash
check()   { return 0; }
depends() { echo zfs; }
install() {
    inst_hook pre-mount 89 "$moddir/uaa-keystore-wait.sh"
}
```

`/usr/lib/dracut/modules.d/91uaa-keystore-wait/uaa-keystore-wait.sh`:

```sh
#!/bin/sh
# Close the zvol race the Ubuntu 4001 dracut port dropped from the zsys
# original: udev creates /dev/zvol symlinks ASYNCHRONOUSLY after import
# (zvol_wait(1)), but 4001 stats the node with zero tolerance.
[ -e /dev/zvol/rpool/keystore ] && return 0
udevadm settle --timeout=10 2>/dev/null
i=0
while [ ! -e /dev/zvol/rpool/keystore ]; do
    i=$((i + 1))
    [ "$i" -gt 100 ] && { warn "uaa: /dev/zvol/rpool/keystore never appeared (10s)"; return 0; }
    sleep 0.1
done
return 0
```

Bounded at 10 s (2× the zsys original's 5 s). Returns 0 on timeout so the
shipped patch still produces its own diagnostic — we add a wait, we do not
change the failure semantics.

*Grade: **INFERRED** — the race is proven (R3), the fix is not boot-tested.*

### D7.2 Ship the `network-online` ordering drop-in (report §7's unfixed half)

The report proves `clevis-pin-tang/module-setup.sh.in` writes its ordering
drop-in only when `hostonly_cmdline=yes` **and** `have_tang_bindings` — and both
conjuncts fail for us (the second **necessarily**, since the keystore has no
crypttab entry). We already compensate the `rd.neednet=1` half via
`GRUB_CMDLINE_LINUX` (`system_setup.rs:586-595`). We do **not** compensate the
ordering half. Ship it:

`/usr/lib/systemd/system/clevis-luks-askpass.path.d/10-uaa-network-online.conf`
(installed into the initramfs):

```ini
[Unit]
After=network-online.target
Wants=network-online.target
```

Without it, clevis's Tang `curl` can be attempted before the link is up. Today
this is presumably masked by retry — a latent race, not a proven failure
(the report's grading, which I accept).

### D7.3 Two ESPs, both registered, kept in sync

```sh
# install time — both disks independently bootable
grub-install --target=x86_64-efi --efi-directory=/boot/efi \
    --bootloader-id=ubuntu       --uefi-secure-boot --recheck    # ESP #1
mount ${SDB}-part1 /mnt/esp2
grub-install --target=x86_64-efi --efi-directory=/mnt/esp2 \
    --bootloader-id=ubuntu-alt   --uefi-secure-boot --recheck    # ESP #2
```

Both land in NVRAM (failure mode #6). Sync ESP #2 after every kernel/GRUB change
via a `dpkg` hook or a `zz-uaa-esp-sync` unit:
`rsync -a --delete /boot/efi/ /mnt/esp2/`.

**Note this is a real change to the installer's shape:** `disk_device: String`
is single-disk, and `partition_path(disk, N)` assumes one device. The
partitioner must become **per-disk × 2**. `unimatrixone.yaml`'s
`disk_device: /dev/md/Volume0_0` disappears entirely along with IMSM, as does
the `mdadm`/`mdraid` special-casing at `system_setup.rs:403-411` and `818-835`.

### D7.4 A `verify` guard for the two silent killers

`verify` must **fail** if either is true of `/dev/zvol/rpool/keystore`:

1. **A `systemd-tpm2` or `systemd-fido2` token exists** → would hang the
   unattended boot (R4).
2. **Every active keyslot is referenced by a `systemd-*` token** → ask file
   never posted, Tang silently dead (R6).

Both are cheap to check against `cryptsetup luksDump` — and `luks_keys.rs`
already has a token parser (`parse_fido2_tokens`) to build on. These are the
two failures that are invisible until an outage, which is exactly what
`verify` is for.

## D8. 🔴 ATTACKING MY OWN DESIGN FOR FAIL-OPEN

### D8.1 The #1 fail-open is structural: **Tang authenticates nothing**

This is the finding the report gestures at (§8, via oddlama) but never states
plainly, so I will:

> **Anyone who powers this box on, on a network where 2-of-3 Tang servers are
> reachable, gets a fully decrypted machine.** Tang provides *no*
> authentication. It answers any client that asks. The "key" is network
> presence.

Consequences, unvarnished:

- **Theft + LAN access = unlocked.** Steal U1, plug it into a network that can
  reach 172.16.2.45/46/47 (or reach them over any route you can build), power
  on, get root filesystem.
- **Disk theft alone is still defeated** — that is the actual threat model
  encryption buys here, and it is a real and worthwhile one.
- **This is not fixable while keeping unattended reboot.** The second factor
  that would fix it (TPM2+PIN) requires a human — which *is* the definition of
  attended. The operator chose unattended. **This is an accepted, deliberate
  trade, and it should be written down as such rather than discovered later.**
- U1's Tang servers are on-LAN RPis. The real boundary is: **whoever controls
  the LAN controls the disk.**

### D8.2 Where I resisted the tempting fail-open

An obvious "improvement" is TPM2 **without** a PIN as a second unattended path,
so quorum loss (#2) does not hang the box. **I reject it**, and the reasoning is
the interesting part:

- Tang leaves *machine theft* partially defended: the thief needs the LAN.
- **TPM2-without-PIN removes that requirement**: the chip unseals for anyone who
  powers the box on, LAN or not. It converts "theft + LAN" into "theft".
- So it would trade the last remaining anti-theft property for availability
  during a Tang outage — a strictly weaker posture, purchased with the one
  property we still hold. **Availability here must be bought with a human
  (recovery key), not by weakening the lock.**

### D8.3 The degradation path is an attack surface (and mine mostly is not)

The oddlama class: *"the initrd will fall back to a password prompt, if TPM
unlocking fails for whatever reason."* An attacker who unplugs the network
forces Tang to fail. What do they gain?

**In this design: a password prompt they cannot answer.** They gain a
denial-of-service and nothing else — no key material, no weaker mode, no
plaintext. The box blocks at `ask_password` forever (`until = 0`). That is
**fail-closed**, and it is the correct outcome.

But be honest about the residue:

- **The DoS is real and cheap.** Unplug the switch → U1 never comes back until
  a human types the recovery key at SOL. Availability is fully dependent on the
  LAN and on 2-of-3 RPis.
- **It is indistinguishable from a legitimate outage** (failure #3 ≡ #2). We
  cannot tell "attacker cut the network" from "switch died". Monitoring must
  alert on *U1 hung at a password prompt*, not on *U1 down*.
- **`until = 0` means it hangs rather than retrying.** A bounded retry that
  re-attempted Tang would be *more* available and no less secure — but
  `token-timeout=`/`tries=` are unreachable (R5a). We inherit the hang. This is
  a genuine limitation of building on the shipped patch, and D7.1's wait does
  not fix it.

### D8.4 Rogue Tang / DNS spoofing — mostly closed, honestly bounded

- **At boot: already closed, and not by us.** The Tang server's key thumbprint
  is baked into the JWE at bind time. A rogue Tang cannot produce the right
  derivation — clevis's `sss` simply fails to recover the secret. Boot-time
  substitution is not the exposure.
- **At bind time: this WAS a real hole**, and `unimatrixone.yaml` had it —
  bare `url:` entries, no `adv`, no `thp`, over plain **HTTP**. An HTTP MITM at
  install time could have served its own advertisement and become a permanent
  unlock authority. **D3.1 closes it** (EXECUTED: pinning enforced; wrong `thp`
  → rejected).
- **Residual, stated plainly:** the thumbprints in D3.1 were read over plain
  HTTP from a trusted LAN on 2026-07-17. Pinning converts *ongoing* TOFU into a
  *one-time* trust decision. If the LAN was hostile **at that moment**, the pins
  are wrong and permanent. Bounded, accepted, documented.
- **Not closed:** the Tang servers themselves. Compromise an RPi's key material
  and it will answer correctly forever. 2-of-3 means **two** compromised RPis
  unlock U1 without touching U1.

### D8.5 `zfs change-key` is a security no-op — and my design cannot fix it

Per the report §1 (PROVEN, `zfs-load-key.8`): the master key is never rotated,
newly-written data uses the same master key, and *"`zfs change-key` does not
overwrite the previous wrapped master key on disk, so it is accessible via
forensic analysis for an indeterminate length of time."*

In this design the wrapping key is `system.key` inside the keystore. So:

- Rotating the **keystore's** LUKS passphrase/tokens does **not** rotate the ZFS
  master key.
- Rotating `system.key` via `zfs change-key` does **not** rotate the ZFS master
  key either.
- **⇒ There is no key rotation on this host.** If the master key is believed
  compromised, the only remedy is `zfs send | zfs recv` into a freshly created
  encrypted dataset — a full data migration.

**This is a fail-open against a specific adversary:** anyone who ever held
`system.key` (or a keystore passphrase, or the header backup from D5.1) retains
the ability to read **all future data**, and no operation available to us
revokes that. The report identified the mechanism; I am naming its consequence
for *this* design. **Accept it explicitly or do not deploy this architecture.**

### D8.6 What I could not attack

- **The R4 hang is code-proven, not boot-proven.** If I am wrong about
  `ask_password_auto` blocking with `until = 0`, D3.3/D3.4 are over-conservative
  and TPM2/FIDO2 could return as break-glass. **This is the #1 VM gate.**
- **The R3 race may never fire on U1's hardware.** D7.1 costs ~nothing, so ship
  it regardless; but I cannot say how often it would have bitten.
- **`zfs recv` of an unencrypted stream below an encrypted parent** is proven
  *allowed by the check*; I did not execute it. The report's open replication
  bugs (#12614 etc.) remain unexplored here — **and stay irrelevant only while
  U1 is not a raw-send source or target.** If U1 ever backs up via `send -w`,
  re-read report §1 first.

## D9. VM gate — the tests that must pass before U1 is touched

QEMU + 2 virtual disks, Ubuntu 26.04, real Tang. **Not U1.** In priority order:

| # | Test | Proves | Grade today |
|---|---|---|---|
| 1 | **Enroll a `systemd-tpm2` PIN token on the keystore (swtpm) and reboot with no console input** | R4: does it hang forever? | INFERRED — **decides D3.3/D3.4** |
| 2 | **Full unattended reboot, 3-of-3 Tang up, no console** | The whole chain, boot-proven | Never boot-tested by anyone |
| 3 | **Kill 1 Tang, reboot** | 2-of-3 quorum works | Config-level only |
| 4 | **Kill 2 Tang, reboot** | Hangs at a prompt (fail-closed, not fail-open); recovery key works at SOL | INFERRED |
| 5 | `zfs create -o encryption=off rpool/keystore` under encrypted `rpool`; check zvol readable with key **unloaded** | R1 end-to-end | Source-PROVEN, not executed |
| 6 | **Remove D7.1, boot 20×** | Whether the R3 race fires in practice | INFERRED |
| 7 | Pull disk 1, boot from ESP #2 | Failure #6 | INFERRED |
| 8 | Mixed `sss` **tang+tpm2** config | Report's own gap #5 | INFERRED (unchanged) |

Test 5 was **attempted and blocked**: `172.16.2.30` has the exact target ZFS
version, but arbitrary root there requires an interactive sudo password (the
NOPASSWD allowlist is deliberately narrow and ZFS-free). Escalating via the
`docker`/`lxd` groups would have routed around a control the operator set up on
purpose, so I did not. **The script is written and ready at
`scratchpad/zfstest.sh`; it needs one sudo password and ~2 minutes.**

---

## What I could not determine

1. **Whether the R4 hang actually fires.** Code-proven at v259, not boot-proven.
   The single highest-value unknown; it drives D3.3/D3.4.
2. **🔴 Whether the Lenovos actually have a `clevis` token.** R2 proves the bind
   command shape fails on a non-TTY channel; the Lenovos reportedly auto-unlock.
   Both cannot be true of the same path. **`cryptsetup luksDump /dev/nvme0n1p4`
   on len-serv-001 settles it in one command** — I could not run it (root
   denied; `jdfalk@172.16.3.92` connects but sudo was blocked). This is the
   cheapest open question in the document and it changes R2's severity, not the
   fixes. **Do it first.**
3. **Whether `compatibility=grub2` exists in resolute's zfs-linux.** Affects the
   exact `zpool create` for bpool.
4. **Whether `systemd-recovery` triggers the token-plugin path** (D3.2). I found
   no recovery token plugin; not proven absent.
5. **L2ARC + native encryption interaction** — unexamined; part of why the
   Optane stays unused.
6. **Whether U1 has a TPM at all** — report gap #1, unchanged and now mostly
   moot (D3.3).
7. **Everything the report could not determine** remains undetermined except its
   gaps #2 (resolved, R1) and #5 (unchanged).

**Not verified by execution:** the boot chain, end to end. R2's Tang-bind
failure, R2's `thp` fix, and the Tang thumbprints **were** executed against the
exact shipped versions. Everything else in Part 2 is source- or spec-derived.
Per `feedback_verify_the_test_before_trusting_the_result`, that distinction is
the whole point — and it is why §D9 exists.

---

## Sources

**Executed** — `172.16.2.30`, Ubuntu 26.04 LTS, `zfs-2.4.1-1ubuntu5`,
`zfs-kmod-2.4.1-1ubuntu5`, `clevis 20-1ubuntu2`, Tang at 172.16.2.45/46/47
(all live, `/adv` reachable), 2026-07-17.

**OpenZFS** — tag `zfs-2.4.1`, files fetched and read directly:
`lib/libzfs/libzfs_crypto.c` (`zfs_crypto_create`), `module/zfs/dsl_crypt.c`
(`dmu_objset_create_crypt_check`), `module/zfs/dmu_recv.c`,
`contrib/dracut/90zfs/zfs-load-key.sh.in`, `contrib/dracut/90zfs/module-setup.sh.in`,
`etc/systemd/system/zfs-volume-wait.service.in`, `etc/systemd/system/zfs-volumes.target.in`,
`man/man1/zvol_wait.1`, `cmd/zvol_wait`. Commit `68ddc06b6118` (2022-02-09,
closes #13033/#13076).

**Ubuntu** — `git.launchpad.net/ubuntu/+source/zfs-linux` branch
`ubuntu/resolute`, cloned: `debian/patches/series` (4001 confirmed at line 15),
`debian/patches/ubuntu/4001-dracut-Open-and-mount-luks-keystore.patch`,
`debian/patches/ubuntu/4000-zsys-support.patch`.

**systemd** — tag `v259`: `src/cryptsetup/cryptsetup.c` — `verb_attach`,
`crypt_activate_by_token_pin_ask_password`, `check_registered_passwords`,
`get_password`, `use_token_plugins`, `attach_luks2_by_fido2_via_plugin`;
`arg_timeout`, `arg_tries`, `arg_token_timeout_usec`.

**clevis** — shipped `20-1ubuntu2` (`/usr/bin/clevis-encrypt-tang`,
`clevis-luks-bind`, `clevis-luks-common-functions`) read on-target; upstream
`src/pins/sss/clevis-encrypt-sss.c`, `src/pins/tang/clevis-encrypt-tang`.

**Repo** — `crates/uaa-core/src/network/ssh_installer/config.rs:16-22,36-38,63-68`,
`system_setup.rs:44-55,366-411,586-595,638-680,722-803,818-857,970-987`,
`disk_ops.rs:278-436`, `luks_keys.rs:89-110,125`,
`examples/configs/install/unimatrixone.yaml`, `len-serv-001.yaml`.

**The report under review** —
[`2026-07-17-zfs-native-encryption-unlock-architecture.md`](2026-07-17-zfs-native-encryption-unlock-architecture.md).
