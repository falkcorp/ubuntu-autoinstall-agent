<!-- file: docs/research/2026-07-17-zfs-native-encryption-recommendation.md -->
<!-- version: 1.3.0 -->
<!-- guid: 99164529-ac14-4478-904e-03bd8b75ade1 -->
<!-- last-edited: 2026-07-17 -->

# Recommendation: what I'd actually build, and where I disagree

Deliverable 3 of the [research brief](../agent-tasks/2026-07-16-zfs-native-encryption-research-prompt.md).
Read after:

1. [Research report](2026-07-17-zfs-native-encryption-unlock-architecture.md) (v1.3.0, corrected)
2. [Adversarial design review](2026-07-17-zfs-native-encryption-design-review.md)

This is my own view, in my own words, including where I disagree with the
specialist.

---

## The one-paragraph answer

**Build native ZFS encryption with the Ubuntu LUKS-keystore zvol, stock
layout, Tang-only for unattended unlock, and a recovery key as the backstop.
Drop TPM2 and FIDO2 from U1's boot path.** The architecture question turned out
to be easy — Ubuntu already ships this exact design for dracut and has
maintained it as recently as three months ago.

**Why drop TPM2/FIDO2 — stated correctly:** *not* because they hang (that is
fixable — see §"The timeout question"), but because **neither can ever be
unattended.** That is spec-level, not configuration: CTAP2 forbids returning an
hmac-secret without a physical touch, and a PIN requires fingers. So neither can
serve the one constraint that actually binds. What they would buy is an
*attended* break-glass path — and **the recovery key already covers that**, at
SOL, with no PCR brittleness and no cost imposed on every boot. Enrolling them
is paying real complexity for a redundant copy of something we already have,
while creating a boot-hang we then have to engineer around.

The config asserts `enroll_tpm2: true` and `expect_fido2: true` today; both
should go.

⚠️ **Two qualifications I added after drafting, because I was wrong twice:**

1. **The hang is code-proven but CONDITIONAL, and our own fleet is evidence
   against it firing** — the Lenovos run the identical config and don't hang.
   See "Order of work" #1.
2. **The hang is fixable, so "it hangs" was never the right reason** to reject
   TPM2/FIDO2. See "The timeout question" below. The conclusion held; the
   argument didn't.

---

## Where I agree with the specialist

I re-verified its headline finding by hand rather than take it on trust, and
it is correct in every detail:

- **R4 (the boot-hang) is real as a code path.** `systemd-cryptsetup` tries
  LUKS2 tokens *before* the password path (`cryptsetup.c:2654`); a
  `systemd-tpm2` PIN token returns `-ENOANO`; that enters `for (;;)` calling
  `ask_password_auto` with `until = 0` — and `until` is 0 because `arg_timeout`
  defaults to `USEC_INFINITY` and `usec_add` saturates (`:2618-2620`). The
  PIN's credential is `cryptsetup.luks2-pin`; clevis matches only
  `Id=cryptsetup:*`. **Tang is never reached.**
  ⚠️ **Two caveats — the specialist overstated this, and so did I at first.**
  (1) It is **conditional**, gated on `use_token_plugins()` *and* on the token
  existing, and **our own fleet is evidence it may not fire today** ("Order of
  work" #1). (2) It is **fixable** via `timeout=` ("The timeout question"), so
  it was never the right *reason* to reject TPM2/FIDO2 — it is a consequence,
  not the argument. The design conclusion (no tokens on the keystore) is right;
  both of us reached it via reasoning that needed repair.
- **R7 demolished my §3(c) argument and it was right to.** I claimed dropping
  IMSM forces ZFS-on-LUKS into two LUKS containers. It doesn't — real mdadm
  RAID1 (which is *not* IMSM fakeraid) keeps one. I dismissed a strawman and
  missed the real alternative. The correct argument is the one the specialist
  supplied: **md under LUKS means ZFS sees one vdev, so it can detect
  corruption but not repair it.** Self-healing needs native mirroring. That is
  a better argument than mine and I've retired mine.
- **Two independent ESPs rather than mdadm RAID1 on the ESP.** Firmware,
  `efibootmgr`, and `fwupd` all write through the raw partition behind md's
  back; a later resync can push the stale half over the fresh one. Corrupting
  the bootloader with the mechanism meant to protect it is a bad trade on a
  host whose whole point is unattended boot.
- **D8.1 is the most valuable paragraph in either document**, and it needs to
  be a written-down accepted risk rather than a discovery: *Tang authenticates
  nothing.* Anyone who powers this box on with LAN reach to 2-of-3 Tang
  servers gets a decrypted machine. **Whoever controls the LAN controls the
  disk.** Disk theft is still defeated — that's the real win — but machine
  theft plus network access is not, and cannot be while unattended reboot is
  the requirement.

---

## The timeout question — "can't we just modify the timeouts?"

Asked by the operator, and it is the right question. **Yes, the hang is
fixable.** I had framed it as disqualifying; it isn't. Here is the full design
space, traced in `systemd/src/cryptsetup/cryptsetup.c` v259.

### `timeout=` does work — and it costs the property we most want

`timeout=` is the lever (`:360-362` → `arg_timeout` → `until` at `:2618-2620`).
With `timeout=30`, `until` becomes a real deadline; `ask_password_auto` returns
`-ETIME`; and — this is the part that matters — the caller does **not** abort,
it `log_debug_errno`s and **falls through to the password loop** (`:2665-2670`),
where clevis answers from Tang. So the boot completes. **PROVEN by reading the
control flow.**

**The catch: `until` is passed to BOTH paths — it is one variable.**

```c
crypt_activate_by_token_pin_ask_password(cd, volume, NULL, until, ...)  // token PIN loop
get_password(cd, ..., until, ..., passphrase_type, ...)                 // ← the one clevis answers
```

`until = 0` is precisely what gives us **"wait patiently at a prompt, forever"**
— the correct behaviour for a remote-operator host. Tang is down at 3am; the box
sits at a prompt; the operator connects to SOL hours later, types the recovery
key, and it boots. Set `timeout=30` and it instead exhausts `arg_tries` (default
3) and drops to an **emergency shell** — converting *a box that waits for you*
into *a box you must rescue*.

**So with a token enrolled you must choose: kill the hang, or keep
patient-waiting. Without a token you get both, free.** That asymmetry is the
whole argument.

### The full option space

| Option | Hang? | Cost | Verdict |
|---|---|---|---|
| **No TPM2/FIDO2 token** | ✅ none | none | ✅ **Recommended** — patient-waiting preserved, Tang direct |
| `timeout=N` + token | ✅ fixed | **N sec every boot** *and* **loses patient-waiting** | ❌ trades a property we want for one we have |
| `SYSTEMD_CRYPTSETUP_USE_TOKEN_MODULE=0` + token | ✅ none | none | ⚠️ Surgical (`:1468` — `use_token_plugins()` → false, path skipped, `until` stays 0). But the token is then **never tried at boot** — enrolled in order not to be used. Marginal. |
| `$PIN` env var in initramfs | ✅ none | **catastrophic** | 🔴 **NEVER — see below** |
| TPM2 **without** PIN | ✅ none | security | ❌ Unseals for anyone who powers it on. Rejected by design review D8.2 — converts *theft + LAN* into *theft*. |

### 🔴 The trap: `$PIN` in the initramfs

`acquire_pins_from_env_variable()` (`:1480-1501`) reads `$PIN` from the
environment and would suppress the prompt entirely. **Do not.** The initramfs
lives on **bpool, which is unencrypted** (GRUB must read it — §7). A PIN stored
there is readable by anyone holding the disk, which destroys the *only* thing a
TPM PIN buys: offline-attack resistance via hardware lockout (§4). It converts
TPM2+PIN into TPM2-without-PIN while looking like it didn't.

### What this changes

**Nothing in the recommendation; everything in the reasoning.** "Don't enroll
TPM2/FIDO2 because it hangs" was wrong — it's fixable. The correct statement is
the one in the one-paragraph answer: **neither can ever be unattended, so
neither can serve the binding constraint, and the recovery key covers the
attended case better.** The hang is then not a reason but a *consequence* — one
more cost of enrolling something that cannot help.

**And note this entire question dissolves under my clevis-`tpm2`-pin proposal
below**, which never creates a systemd token at all — so there is no token path
to time out, and `until = 0` is kept.

---

## Where I disagree — one substantive gap

### The specialist rules out TPM2 too broadly. Clevis's `tpm2` **pin** is not systemd's `tpm2` **token**.

R4 proves that a **`systemd-tpm2` token** on the keystore hangs the boot. I
verified it. But the design generalises that to "no TPM2, period" — and I
don't think that follows, for a reason the review itself establishes.

The review's own R6 identifies why clevis survives at all: **its token type is
foreign to systemd.** Clevis writes `{"type":"clevis",...}`; systemd's plugins
filter on `systemd-tpm2` / `systemd-fido2` / `systemd-pkcs11` /
`systemd-recovery`. libcryptsetup has **no handler for a `clevis` token**, so
`crypt_activate_by_token_pin(..., CRYPT_ANY_TOKEN, ...)` skips it and never
returns `-ENOANO`.

**That property applies to clevis's `tpm2` pin exactly as it applies to its
`tang` pin.** A clevis tpm2 binding lives inside the same `clevis` JWE token.
It should therefore be invisible to systemd's token loop — no `-ENOANO`, no
PIN prompt, no hang. It is unsealed by `clevis-luks-askpass` answering the
ordinary `Id=cryptsetup:` prompt, exactly like Tang.

**If that holds, it buys a strictly better posture at zero security cost.**
Today's plan is `sss` with `t=2` over 3 Tang servers. Add the tpm2 pin as a
fourth share:

```json
{"t":2,"pins":{"tang":[{"url":"...45","thp":"..."},{"url":"...46","thp":"..."},{"url":"...47","thp":"..."}],"tpm2":{}}}
```

| Scenario | 2-of-3 Tang (planned) | 2-of-{3 Tang + TPM2} (proposed) |
|---|---|---|
| All Tang up | ✅ unlocks | ✅ unlocks |
| **1 Tang down** | ✅ unlocks | ✅ unlocks |
| **2 Tang down** | ❌ **hangs — human needed** | ✅ **unlocks** (TPM2 + 1 Tang) |
| Machine stolen, no LAN | ✅ stays locked | ✅ **stays locked** (TPM2 alone = 1 share, needs 2) |
| Disk pulled | ✅ stays locked | ✅ stays locked |

**This is the rare change that improves availability without weakening the
threat model** — and that is precisely because of the threshold. It does not
fall to the specialist's own D8.2 objection ("TPM2-without-PIN converts *theft
+ LAN* into *theft*"), because a lone TPM2 share **cannot reach `t=2`**. The
LAN requirement survives intact. D8.2's rejection is correct for TPM2 as an
*independent* unlock path; it does not reach TPM2 as a *sub-threshold share*.
I think the design conflated those two.

**Two hard preconditions, and I will not assert past them:**

1. 🔴 **We do not know whether U1 has a TPM module at all.** The X10DSC+ has a
   20-pin LPC `JTPM1` header, but it **ships empty** — the TPM is a
   separately-purchased AOM-TPM-9665V. Nothing in this repo or any memory
   records one being bought or fitted. `enroll_tpm2: true` may be asserting
   something physically impossible. **This is the single highest-value unknown
   and it needs `tpm2_getcap` on the host.** If there's no TPM, this entire
   disagreement is moot and the specialist's design stands unchanged.
2. ⚠️ **The mixed tang+tpm2 `sss` config is INFERRED, not proven.** The format
   spec is unambiguous but **no worked upstream example was found** — the
   research flagged this explicitly. Per the brief's own standard ("don't tell
   me something works because it should work"), **this must be VM-tested
   before it is trusted.**

So: I recommend this as a **VM experiment**, not as a design change to adopt on
paper. If either precondition fails, build exactly what the specialist
specified.

### A smaller one: FIDO2 isn't "break-glass", it's *gone*

The research report and the brief both treat FIDO2 as demoted to break-glass.
Following R4 to its conclusion, that is too generous. A **`systemd-fido2`
token on the keystore hangs the boot for the same structural reason as TPM2**
(the plugin needs a PIN → `-ENOANO`, or needs a touch → blocks). And FIDO2
cannot traverse IPMI SOL — you cannot tap a capacitive sensor over a serial
console. So for a remote operator, FIDO2 offers **nothing the recovery key
doesn't**, while costing a boot-hang.

**`expect_fido2: true` in `unimatrixone.yaml` should become `false` for U1.**
Not deferred — removed. That is a real config change with a real consequence,
and it should be a deliberate decision rather than an omission someone
rediscovers at 3am.

---

## What I'd actually build

1. **Stock Ubuntu layout, no deviation.** Encryption root = the bare pool
   `rpool`; keystore zvol = `rpool/keystore` with `encryption=off`;
   `keylocation=file:///run/keystore/rpool/system.key`. Any other layout means
   carrying a patched `zfs-load-key.sh` across every `zfs-linux` update — and
   someone already hit exactly that bug on 25.10 with an `rpool/enc` root.
2. **Keystore token set: clevis (Tang, `thp`-pinned) + passphrase + recovery
   key. Nothing else.** No `systemd-tpm2`, no `systemd-fido2`. This is the
   difference between a box that comes back at 3am and one that doesn't.
3. **Pin Tang thumbprints.** Fixes the fragile bind *and* closes the
   trust-on-first-use hole — `unimatrixone.yaml` currently lists Tang URLs with
   no `adv`/`thp` pinning, so a rogue Tang on the LAN is presently trusted.
   Two wins, one change.
4. **Ship the `91uaa-keystore-wait` dracut hook.** The Ubuntu dracut port
   dropped the zvol wait loop that the initramfs-tools/zsys version has, and
   udev creates `/dev/zvol/*` links asynchronously. Upstream ships
   `zfs-volume-wait.service` for precisely this. Without the hook this is a
   race we'd lose intermittently — the worst failure class.
5. **Ship the `network-online` ordering drop-in.** The research proved clevis's
   auto-wiring never fires on Ubuntu (`hostonly_cmdline` is empty), so
   `clevis-luks-askpass` isn't ordered after `network-online.target`. We already
   compensate the `rd.neednet=1 ip=dhcp` half by hand in
   `system_setup.rs:586-595` — this is the other half.
6. **Two ESPs, both registered in NVRAM, synced on every bootloader change.**
7. **Write down the accepted risk**: whoever controls the LAN controls this
   disk. That is the deliberate price of unattended reboot.

## Order of work — cheapest disambiguation first

1. 🎯 **`cryptsetup luksDump` on `len-serv-001`.** Free, no U1 needed, and it
   settles **two** findings at once — including one of mine.

   **I checked the Lenovo configs after drafting this, and they undercut my own
   headline.** `len-serv-001/002/003.yaml` set **identical** fields to U1 —
   `enroll_tpm2: true`, `tpm2_pin: REPLACE_AT_PLACE_TIME`, `expect_fido2: true`,
   `initramfs_type: dracut` — and they **auto-unlock in production**. If R4
   fired unconditionally on that config, they would hang. **So R4 is
   conditional**, gated on `use_token_plugins()` *and* on a `systemd-tpm2`
   token actually existing (`cryptsetup.c:1449-1478`).

   **My hypothesis: TPM2 enrollment silently never succeeds on the Lenovos**
   (the PIN is an unsubstituted placeholder; `expect_fido2` only "records
   intent"; FIDO2 is enrolled manually). If so, **the fleet already runs the
   Tang-only token set I'm recommending — by accident — and "fixing" TPM2
   enrollment is what would break it.**

   Read the `Tokens:` section:
   - `clevis` present, **no** `systemd-tpm2` ⇒ hypothesis confirmed; R4 real
     but latent; enrollment quietly broken; my recommendation is what's already
     deployed.
   - `systemd-tpm2` present on a host that auto-unlocks ⇒ **R4 does not fire in
     our flow and I am wrong** — re-open correction #1.

   This is `feedback_verify_the_test_before_trusting_the_result` biting me
   specifically: I verified the *code path* by hand and was ready to call it
   settled. **Proving a code path exists is not proving it executes.** The
   recommendation holds either way — which is exactly why it was safe to be
   wrong here, and why I'd rather flag it than let it ship as fact.
2. **VM gate, not U1.** Every claim in all three documents is source-, spec-,
   or package-derived. **Nothing here is boot-proven.** The R4 hang is the
   test that matters most: it decides whether TPM2/FIDO2 can ever come back.
3. **Determine whether U1 has a TPM** — when hardware access is authorised.
   Decides my disagreement above.
4. **Only then** touch U1.

## The thing I'd want challenged in my own recommendation

My clevis-tpm2-pin proposal rests on a **negative**: that libcryptsetup has no
handler for a `clevis` token and therefore skips it. I verified that clevis
registers no token module (`grep crypt_token_register|token_open clevis/src` →
empty) and that systemd filters on its own type strings. But "I found no
handler" is not the same as "the code path cannot fire", and I have not
executed it. **If a clevis tpm2 binding did somehow surface as a token
libcryptsetup tries, it would reintroduce the R4 hang — the exact failure I'm
claiming to avoid.** That is the first thing to check in the VM, and I would
rather name it than let it be found later.

---

## Honest summary of the state of this work

| | Status |
|---|---|
| Architecture decision | **Settled** — native ZFS + Ubuntu keystore zvol, stock layout |
| Dataset layout / circularity | **Resolved, proven twice independently** |
| Unattended path | **Settled** — Tang only; everything else is attended |
| TPM2 | **Open** — hangs as a systemd token (**code-proven, conditional, and NOT proven to fire in our flow** — the Lenovos run the same config and don't hang); *may* work as a clevis sss share (unproven); **hardware presence unknown** |
| FIDO2 | **Recommend removing from U1 entirely** |
| Tang bind reliability | **Open** — fragile shape proven; Lenovo contradiction unresolved; one `luksDump` settles it |
| Enrollment integrity | 🔴 **Newly suspected** — TPM2/FIDO2 enrollment may be silently failing fleet-wide. Would mean the fleet is Tang-only by accident, and that `verify`'s `expect_fido2` check is not catching it. **Independent of this design; worth its own look.** |
| Boot-proof | **None. Nothing here has been booted.** VM gate required. |
