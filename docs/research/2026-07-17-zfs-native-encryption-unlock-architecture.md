<!-- file: docs/research/2026-07-17-zfs-native-encryption-unlock-architecture.md -->
<!-- version: 1.1.0 -->
<!-- guid: 4cd78aaf-cb57-494b-ac6d-f48193b7cb88 -->
<!-- last-edited: 2026-07-17 -->

# Research Report: ZFS Native Encryption + Unlock Architecture

Answers the research brief in
[`docs/agent-tasks/2026-07-16-zfs-native-encryption-research-prompt.md`](../agent-tasks/2026-07-16-zfs-native-encryption-research-prompt.md).
Target host: `unimatrixone` (U1), Supermicro X10DSC+, Ubuntu 26.04 "resolute".

**Document type note.** This is a *research report* — a third artifact type
alongside [status reports](../process/status-reports.md) and
[executive summaries](../process/executive-summaries.md). Neither existing
convention fits an evidence document (there is no "Shipped this session"
table here — nothing shipped). It therefore adopts the **status-report
register** as the closest match, per that convention's own allowance for
"additional freeform sections (setup notes, config dumps, findings)":
internal/engineer audience, `file:line` references and jargon used freely,
no stakeholder polish. It lives in `docs/research/` because it is neither
a status snapshot nor a design spec — it is the evidence base a spec will
be built on.

---

## TL;DR

Native ZFS encryption is viable on U1, and the path to it is **already
shipped by Ubuntu** — we do not have to invent a shim. But it does not work
the way the brief assumed, and the reasons matter:

- **Clevis and `systemd-cryptenroll` are both LUKS2-only.** Native ZFS
  encryption can reuse *neither* directly. This is proven at source level in
  both projects, and it is the single fact that structures every design
  choice below.
- **`keylocation` cannot call a script.** OpenZFS hard-codes exactly three
  URI schemes (`file`, `https`, `http`) in a compile-time table. There is no
  `exec://`, no pipe, no plugin. A "keylocation points at a script" shim is
  **impossible**, not merely unsupported.
- **Ubuntu 26.04 already ships the LUKS-keystore design, for dracut, as of
  April 2026** (three months ago). The brief's §3 "my original thought" turns
  out to be the *distro-supported path*, not a workaround.
- **The keystore is a zvol inside rpool**, not a new partition. It therefore
  inherits ZFS mirroring for free — which dissolves the "another ESP-style
  thing to duplicate" objection before it is raised.
- **Tang is the only unattended-reboot path.** Not a preference — FIDO2
  cannot be unattended at the *spec* level (CTAP2 forbids returning an
  hmac-secret without a user-presence touch), and TPM2+PIN needs a human to
  type the PIN.
- **The honest catch:** with a keystore, native encryption's *unlock*
  security is **identical** to today's ZFS-on-LUKS. The real wins are
  elsewhere (see §3), and one of them is large and specific to the
  already-locked IMSM decision.

The dangerous area is **not** at-rest encryption — it is `zfs change-key`
combined with raw send/recv, where OpenZFS has **open** correctness bugs
today.

---

## 🔴 CORRECTIONS — read before acting on this report

This report was adversarially reviewed. **The review found four errors, one of
them critical.** Full analysis:
[`2026-07-17-zfs-native-encryption-design-review.md`](2026-07-17-zfs-native-encryption-design-review.md).
Corrections are applied inline below; they are summarised here because acting
on the uncorrected text would break the host.

| # | What this report said | Correction |
|---|---|---|
| **1** 🔴 | §8 treats **TPM2+PIN as a viable break-glass rung** | **REFUTED — it is a boot-hang, not a fallback.** A `systemd-tpm2` PIN token on the keystore is tried *before* the password path, returns `-ENOANO`, and enters `for(;;)` on an ask-password with **`until = 0` (no deadline)**. Its credential is `cryptsetup.luks2-pin`; clevis only matches `Id=cryptsetup:*`, so **clevis never answers and Tang is never reached**. **Verified by hand this session** in `systemd/src/cryptsetup/cryptsetup.c` v259 (`:103`, `:1519-1568`, `:2618-2620`, `:2654-2665`). **With `unimatrixone.yaml`'s current `enroll_tpm2: true` + `tpm2_pin`, U1 would hang forever on exactly the unattended reboot it must survive.** ⇒ **Enroll no TPM2/FIDO2 token on the keystore.** |
| **2** 🔴 | §5/§8 prescribe **`headless=`** as the fix for the fail-open | **Wrong twice.** (a) clevis unlocks *by answering* the very interactive prompt `headless=` suppresses — it would destroy Tang. (b) It is **unreachable anyway**: the Ubuntu patch calls `systemd-cryptsetup attach <name> <dev>` with no CONFIG argument (`argv[4]`), so no crypttab option applies. Verified against the patch body and `cryptsetup.c:2563`. |
| **3** 🔴 | §5: "`token-timeout=` defaults to 30s, after which authentication via password is attempted — **the automatic degradation path**" | `arg_token_timeout_usec` is real but **unsettable here**, and it does **not** bound the token-plugin PIN loop (which uses `until`). **That automatic degradation path does not exist in this design.** |
| **4** ⚠️ | §3(c): the **enrollment-surface** argument for native encryption | **False as stated** — real mdadm RAID1 (≠ IMSM fakeraid) under LUKS keeps **one** container. Conclusion survives, reasoning retired. See §3(c). |

**✅ And one gap CLOSED — the open question is resolved, in the design's
favour.** Gap #2 ("does `rpool/keystore` inherit encryption — is it
circular?") was resolved **independently by two agents that agreed**:

- **An unencrypted child under an encrypted parent is LEGAL.** OpenZFS commit
  [`179374cc`](https://github.com/openzfs/zfs/commit/179374ccf27eef6932777cf29ae15e1cfbf85b91)
  — *"Allow unencrypted children of encrypted datasets"* (fixing
  [#8737](https://github.com/openzfs/zfs/issues/8737)) — *"some legitimate
  reasons have been brought up for this behavior to be allowed. This patch
  simply removes this limitation from all code paths that had checks for it."*
  The removed guards are **absent** from Ubuntu's shipped 2.4.1 tree. PROVEN.
- **Ubuntu's actual creation code proves the layout** — curtin `block/zfs.py`:
  `zfs_create(self.poolname, "keystore", {"encryption": "off"}, ...)`, with
  `encryption`/`keylocation`/`keyformat` emitted as **`-O`** flags on
  `zpool create` ⇒ **the encryption root is the bare pool `rpool`**, and
  `rpool/keystore` is an explicitly-unencrypted sibling of `rpool/ROOT`.
- **There is no circularity, and `encryption=off` is not plaintext.** The two
  layers are stacked, not nested: the zvol's blocks are **LUKS ciphertext**.
  The chain is `Tang/passphrase → LUKS → system.key → ZFS-native rpool`.
  `encryption=off` disables only the *ZFS-native* layer for that zvol.
- ⚠️ **But this report misdiagnosed the dracut port's real bug.** It is not a
  layout error — it is a **zvol race**: the dracut port dropped the wait loop
  the initramfs-tools/zsys version has, and udev creates `/dev/zvol/*` links
  **asynchronously** (upstream ships `zfs-volume-wait.service` for exactly
  this). The `$ENCRYPTIONROOT`-vs-`find` asymmetry is a *separate*, real
  robustness bug that bites only non-stock layouts (someone hit it on 25.10
  with an `rpool/enc` root). **Stock layout + our own wait hook is the fix.**

---

## How to read this report

Every claim carries a grade. This is not decoration — the brief explicitly
asked for evidence over conclusions, and the
`feedback_verify_the_test_before_trusting_the_result` lesson applies
directly.

| Grade | Meaning |
|---|---|
| **PROVEN** | A primary source is quoted verbatim, or source code was read at a pinned tag. |
| **INFERRED** | Reasoning from proven facts. The reasoning is shown so it can be attacked. |
| **COULD NOT VERIFY** | Searched, not found. Recorded as a gap, not filled with a guess. |

**Verification method.** Research was fanned out across specialist agents,
but the load-bearing claims were **re-verified by hand** against pinned
upstream sources rather than trusted as paraphrase. Specifically re-checked
in this session:

- `uri_handlers[]` in `libzfs_crypto.c` at tag `zfs-2.4.1` — confirmed verbatim.
- The `isatty()`/stdin fallback in `get_key_material()` — confirmed verbatim.
- `4001-dracut-Open-and-mount-luks-keystore.patch` — cloned
  `git.launchpad.net/ubuntu/+source/zfs-linux` branch `ubuntu/resolute`,
  confirmed the patch exists **and is applied** (`debian/patches/series:15`).
- `clevis-luks-askpass` — read in full; confirmed it keys off the ask-file's
  `Id=cryptsetup:` field, not `/etc/crypttab`.
- `zfs-linux 2.4.1-1ubuntu5` in resolute — confirmed via Launchpad API.

Where a subagent's claim could not be independently re-checked, it is graded
on the strength of the citation it supplied, and said so.

---

## Bottom line up front: the five findings that structure the design

### Finding 1 — The whole unlock toolchain is LUKS2-only. PROVEN.

Both halves of the ecosystem refuse non-LUKS2 targets, and both do so
structurally rather than by policy:

`systemd-cryptenroll(1)`, DESCRIPTION, verbatim (identical at v258 and v259):

> "The tool supports only LUKS2 volumes, as it stores token meta-information
> in the LUKS2 JSON token area, which is not available in other encryption
> formats."

Backed by source — this is a hard-coded constant, not an extension point
([`cryptenroll.c#L785`](https://github.com/systemd/systemd/blob/v258/src/cryptenroll/cryptenroll.c#L785)):

```c
r = crypt_load(cd, CRYPT_LUKS2, NULL);
if (r < 0)
        return log_error_errno(r, "Failed to load LUKS2 superblock of %s: %m", arg_node);
```

Clevis is the same story
([`clevis-luks-bind`](https://github.com/latchset/clevis/blob/master/src/luks/clevis-luks-bind)):

```bash
if ! luks_type="$(clevis_luks_type "${DEV}")"; then
    echo "${DEV} is not a supported LUKS device" >&2
    exit 1
fi
```

**Consequence:** every enrollment concept we currently rely on — keyslots,
`--wipe-slot`, recovery keys, FIDO2 tokens, TPM2 tokens — is a *LUKS2
keyslot/token concept*. ZFS has no analogue. If we go native, we either
rebuild all of it or keep a LUKS2 object somewhere to hold it.

There is not even an upstream *request* for this: `gh search issues --repo
systemd/systemd "cryptenroll zfs"` returns zero results. Clevis's equivalent
([issue #218](https://github.com/latchset/clevis/issues/218), "Use clevis for
ZFS native encryption passphrase") has been **open since 2020-08-02**, and
its implementation attempt
([PR #373](https://github.com/latchset/clevis/pull/373)) was **never merged**
and is stale since 2024. Nobody is coming to fix this.

### Finding 2 — `keylocation` cannot exec a script. PROVEN at source, re-verified by hand.

The brief asks (§2) whether a shim could be "a `keylocation` pointing at a
script". It cannot. The scheme table is compiled in
([`libzfs_crypto.c:77-82`, tag `zfs-2.4.1`](https://github.com/openzfs/zfs/blob/zfs-2.4.1/lib/libzfs/libzfs_crypto.c)) —
this is the exact output of a hand re-verification this session:

```c
static zfs_uri_handler_t uri_handlers[] = {
	{ "file", get_key_material_file },
	{ "https", get_key_material_https },
	{ "http", get_key_material_https },
	{ NULL, NULL }
};
```

Unknown schemes fail closed: `get_key_material()` sets `ret = ENOTSUP` →
`"URI scheme is not supported"` (`libzfs_crypto.c:726,742`, re-verified).
The man page grammar agrees (`zfsprops.7`):

> `keylocation=prompt|file:///absolute/file/path|https://address|http://address`

**But there are two supported shim points**, and this is what makes the whole
design possible:

**(a) `keylocation=prompt` + non-TTY stdin.** Man page, verbatim:

> "If `prompt` is selected... **If stdin is a TTY, then ZFS will ask for the
> key to be provided. Otherwise, stdin is expected to be the key to use and
> will be processed as such.**"

Re-verified in source this session (`libzfs_crypto.c:708-719`):

```c
case ZFS_KEYLOCATION_PROMPT:
    if (isatty(fileno(stdin))) {
        can_retry = keyformat != ZFS_KEYFORMAT_RAW;
        ret = get_key_interactive(hdl, fsname, keyformat, do_verify, newkey, &km, &kmlen);
    } else {
        /* fetch the key material into the buffer */
        ret = get_key_material_raw(stdin, keyformat, &km, &kmlen);
    }
```

So `anything | zfs load-key -L prompt <dataset>` works, and is documented.
This is not a hack — **it is what upstream's own initramfs script does**
(`plymouth ask-for-password | zfs load-key`).

**(b) `file://` to a path that something else populates.** Which is exactly
what Ubuntu chose. See Finding 3.

### Finding 3 — Ubuntu already ships the answer, for dracut, as of three months ago. PROVEN.

This is the most consequential finding in the report, and it was verified by
cloning the Ubuntu source package directly rather than trusting a summary.

`zfs-linux 2.4.1-1ubuntu5` (resolute, Release pocket, confirmed via Launchpad
API) carries
`debian/patches/ubuntu/4001-dracut-Open-and-mount-luks-keystore.patch`,
**applied** at `debian/patches/series:15`. The changelog entry, verbatim:

```
zfs-linux (2.4.1-1ubuntu5) resolute; urgency=medium

  * ubuntu/4001-dracut-Open-and-mount-luks-keystore.patch: Let zfs Dracut module
    depend on systemd-cryptsetup (LP: #2148282)

 -- Benjamin Drung <bdrung@ubuntu.com>  Tue, 14 Apr 2026 16:18:01 +0200
```

The patch header states the problem it fixes, verbatim:

```
Booting an encrypted ZFS system with dracut fails:

dracut-pre-mount[817]: Warning: ZFS: Key /run/keystore/rpool/system.key for rpool hasn't appeared. Trying anyway.
dracut-pre-mount[863]: Key load error: Failed to open key material file: No such file or directory
[FAILED] Failed to mount sysroot.mount - /sysroot.

4000-zsys-support.patch enhances `contrib/initramfs/scripts/zfs` to open
and mount luks keystore for any pools using one. Port this Ubuntu
keystore convention to Dracut.

Bug-Ubuntu: https://launchpad.net/bugs/2070066
```

And the mechanism itself:

```sh
_open_and_mount_luks_keystore() {
    pool="$1"
    keyfile="$2"

    ks="/dev/zvol/$pool/keystore"
    if [ ! -e "$ks" ]; then
        echo "Error: $ks does not exist." >&2
        return 1
    fi

    systemd-cryptsetup attach "keystore-${pool}" "${ks}"

    dev="/dev/mapper/keystore-${pool}"
    if [ ! -e "$dev" ]; then
        echo "Error: $dev does not exist." >&2
        return 1
    fi

    keypath="${keyfile%/*}"
    mkdir -p "${keypath}"
    mount -o discard "${dev}" "${keypath}"
}
```

triggered by a `keylocation` path match:

```sh
    KEYLOCATION="$(zfs get -Ho value keylocation "${ENCRYPTIONROOT}")"
    case "$KEYLOCATION" in
        "file:///run/keystore/${ENCRYPTIONROOT}/"*)
            _open_and_mount_luks_keystore "${ENCRYPTIONROOT}" "${KEYLOCATION#file://}"
            ;;
    esac
```

**Read that carefully — `ks="/dev/zvol/$pool/keystore"`.** The keystore is a
**zvol inside rpool**, not a partition. That works because ZFS native
encryption leaves *pool structure* unencrypted (§1): `zpool import` needs no
key, so the zvol appears as a block device, and only then is it LUKS-opened.

**Why this matters more than it first appears** (INFERRED, but the reasoning
is short): a zvol inside a mirrored rpool is **mirrored by ZFS itself**. The
advisor-flagged objection — "a LUKS keystore is a new small partition that
must survive a disk death, same availability problem as the ESP" — **does
not apply to this design**. There is no new partition. The ESP remains the
only layer needing duplication, exactly as
`project_storage_arch_drop_imsm_native_zfs` already concluded.

### Finding 4 — TPM2 cannot do the Apple Secure Enclave thing. PROVEN, not assumed.

The brief said: *"Be honest — I suspect the answer is 'no, it gets released,'
but I want it proven, not assumed."* It is proven. §4 has the full evidence;
the short version is three independent proofs:

1. **`TPM2_Unseal` returns the secret to the caller.** TCG Part 3 §12.7,
   Table 32: `TPM2B_SENSITIVE_DATA outData` — "unsealed data / **Size of
   outData is limited to be no more than 128 octets.**" A 128-byte ceiling
   cannot stream disk blocks; it hands back a *key*.
2. **The TPM is not in the storage DMA path.** It is an LPC/SPI *slave*
   peripheral (TCG PTP §7.4.1 mandates a 10–24 MHz SPI clock). It cannot
   master a DMA transfer. Apple's AES engine, by contrast, is "**built into
   the direct memory access (DMA) path between the NAND flash storage and
   main system memory**" and "**never exposes the unwrapped key to
   software**" (Apple Platform Security).
3. **The key demonstrably lives in kernel memory.** dm-crypt holds it in the
   kernel keyring; ZFS holds the master key in the kernel.

**And it is not even transient** (INFERRED, strongly): the key stays resident
for the life of the mapping, because every I/O must be decrypted. "The key is
in RAM only briefly" is false.

**Practical read:** the key being in kernel RAM is unavoidable and is *not a
design flaw to engineer around*. Accept it. The security actually purchasable
is a **TPM2 PIN** backed by hardware anti-hammering — see §4 for why that is
worth more than it looks, and for a default that has silently changed.

### Finding 5 — Tang is the only unattended path. PROVEN.

For U1's binding constraint (§8), the enrolled methods sort cleanly:

| Method | Unattended? | Why |
|---|---|---|
| **Tang (network)** | ✅ **Yes** | Non-interactive HTTP; the only one that qualifies |
| TPM2 **without** PIN | ⚠️ Yes, but | Unseals with no human — *and* see the §4 warning about the systemd 258 PCR default |
| TPM2 **+ PIN** | ❌ No | A human must type the PIN |
| **FIDO2 / YubiKey** | ❌ **No — impossible** | CTAP2 spec forbids hmac-secret without user presence |
| PIV / PKCS#11 | ❌ No (via systemd) | `CKF_LOGIN_REQUIRED` is hard-coded; always prompts |
| Recovery passphrase | ❌ No | A human must type it |

The FIDO2 result is stronger than "not configured for it" — it is
categorical. See §6.

---

## 1. Native ZFS encryption — how it actually works

**Version pin.** Ubuntu 26.04 "resolute" ships **`zfs-linux 2.4.1-1ubuntu5`**
(Release pocket) — PROVEN, re-verified this session via the Launchpad API.
Every bug-status claim below is mapped to *that* version. For comparison:
24.04 noble = 2.2.2, 25.10 questing = 2.3.4, 26.10 stonking = 2.4.2.

> **Do not reason from the version string alone.** Ubuntu cherry-picks fixes
> without bumping the upstream version, so the authority is the patch series,
> not the number. Resolute's `debian/patches/ubuntu/` was inspected directly
> (this session): 11 patches, **none touching encryption correctness** — they
> are zsys support, the dracut keystore port, bpool-upgrade disabling, kernel
> 7.0 fixes, and build plumbing.

### Key hierarchy and encryption roots

`zfs-load-key.8` §Encryption, verbatim (tag `zfs-2.4.1`):

> "Creating an encrypted dataset requires specifying the `encryption` and
> `keyformat` properties at creation time... After entering an encryption key,
> the created dataset will become an encryption root. Any descendant datasets
> will inherit their encryption key from the encryption root by default,
> meaning that loading, unloading, or changing the key for the encryption root
> will implicitly do the same for all inheriting datasets."

The **wrapping key vs master key** distinction is where the sharpest
under-appreciated fact lives. Same man page, `change-key` section, verbatim:

> "If the user's key is compromised, `zfs change-key` does not necessarily
> protect existing or newly-written data from attack. **Newly-written data
> will continue to be encrypted with the same master key as the existing
> data.** The master key is compromised if an attacker obtains a user key and
> the corresponding wrapped master key. Currently, `zfs change-key` **does not
> overwrite the previous wrapped master key on disk, so it is accessible via
> forensic analysis for an indeterminate length of time.**"

**`zfs change-key` rotates the wrapping key only. The master key is immortal
for the life of the dataset.** PROVEN. This has a direct operational
consequence for us: *there is no true key rotation for ZFS native encryption*.
Compare LUKS, where `cryptsetup-reencrypt` genuinely rotates the volume key.
If a key is believed compromised, the only real remedy is
`zfs send | zfs recv` into a freshly-created encrypted dataset — i.e. a
full data migration.

Properties (`zfsprops.7`): `keyformat=raw|hex|passphrase`. Raw/hex keys "must
be **32 bytes long** (regardless of the chosen encryption suite) and must be
randomly generated." Passphrases are 8–512 bytes through PBKDF2, default
`pbkdf2iters=350000`, minimum 100000.

Inheritance is unusual and worth internalising: `keystatus`, `keyformat`,
`keylocation`, `pbkdf2iters` "**do not inherit like other ZFS properties**
and instead use the value determined by their encryption root." Tracked by
the read-only `encryptionroot` property. One exception: "**clones will always
use their origin's encryption key.**"

### What IS and IS NOT encrypted — the precise metadata leak

`zfs-load-key.8` §Encryption, verbatim (text identical at 2.4.1 and master):

> "ZFS will encrypt file and volume data, file attributes, ACLs, permission
> bits, directory listings, FUID mappings, and `userused`/`groupused` data.
> **ZFS will not encrypt metadata related to the pool structure, including
> dataset and snapshot names, dataset hierarchy, properties, file size, file
> holes, and deduplication tables** (though the deduplicated data itself is
> encrypted)."

So the leak is exactly: **dataset names, snapshot names, dataset hierarchy,
properties, file sizes, file holes, dedup tables.** Two nuances the brief's
phrasing didn't include:

- **Directory listings and file attributes ARE encrypted** — the leak is
  narrower than "folder names" suggests. What leaks is the *dataset* tree,
  not the *directory* tree.
- **"File holes" leaks the sparseness map of every file.** This is the least
  intuitive item on the list.

The brief pre-accepted this leak and asked not to be argued out of it on
those grounds. Accepted — **and it is load-bearing in our favour**: it is
*precisely because* pool structure is unencrypted that `zpool import` works
without a key, which is what makes the keystore-zvol design possible at all
(Finding 3).

Other documented limitations from the same section, all PROVEN and all
relevant:

- "Encrypted datasets may not have `copies=3` since the implementation stores
  some encryption metadata where the third copy would normally be."
- "Encryption is applied after compression so compression ratios are
  preserved... datasets may be vulnerable to a **CRIME-like attack**."
- Encrypted checksums = "128 bits of the user-chosen checksum and 128 bits of
  MAC."
- Datasets "can be scrubbed, resilvered, renamed, and deleted **without the
  encryption keys being loaded**." — operationally excellent for us: routine
  pool maintenance never needs the key.

### What actually prompts at boot

**There is no `zfs-load-key.service`.** PROVEN — `git ls-tree -r zfs-2.4.1 |
grep -c zfs-load-key.service` → 0. Root key loading is done by an initramfs
script, not a systemd unit. (`zfs-mount-generator` exists but handles
mounts/`keyloaded` ordering for *non-root* datasets.)

On dracut, the relevant file is `contrib/dracut/90zfs/zfs-load-key.sh`,
registered via `inst_hook pre-mount 90`. Details in §7.

For completeness, the initramfs-tools path (which we are **not** using —
see §7) has a sanctioned drop-in at `/etc/zfs/initramfs-tools-load-key.d/*`,
added in OpenZFS 2.2.0 by commit `6e015933`. **This is a trap for anyone
reading general ZFS-unlock advice: that drop-in does not exist on dracut.**
Most blog posts about "ZFS + Tang" use it.

### Raw send (`-w`) and inheritance

`zfs-send.8`, verbatim:

> "`-w, --raw` For encrypted datasets, send data exactly as it exists on disk.
> This allows backups to be taken even if encryption keys are not currently
> loaded. The backup may then be received on an untrusted machine since that
> machine will not have the encryption keys to read the protected data or
> alter it without being detected. **Upon being received, the dataset will
> have the same encryption keys as it did on the send side, although the
> `keylocation` property will be defaulted to `prompt` if not otherwise
> provided.** For unencrypted datasets, this flag will be equivalent to
> `-Lec`. Note that **if you do not use this flag for sending encrypted
> datasets, data will be sent unencrypted and may be re-encrypted with a
> different encryption key on the receiving system, which will disable the
> ability to do a raw send to that system for incrementals.**"

Two operational traps in that one paragraph: `-w` does **not** preserve
`keylocation` (resets to `prompt`), and a **single non-raw send permanently
poisons that target for future raw incrementals**.

### Known correctness bugs — the highest-risk area

This is where the brief's suspicion ("I've heard there are correctness bugs
around raw send/recv and `zfs change-key`") is **correct and current**.

**Fixed, and present in 26.04's 2.4.1:**

| Issue | Title | State | Fix | In 2.4.1? |
|---|---|---|---|---|
| [#10523](https://github.com/openzfs/zfs/issues/10523) | "Raw send on encrypted datasets does not work when copying snapshots back" | closed 2022-01-21 | PR [#12981](https://github.com/openzfs/zfs/pull/12981) | ✅ |
| [#12594](https://github.com/openzfs/zfs/issues/12594) | "Sometimes raw send on encrypted datasets does not work when copying snapshots back" | closed 2022-01-21 | PR #12981 | ✅ |
| [#6845](https://github.com/openzfs/zfs/issues/6845) | "Encrypted Indirect BPs erroneously MAC byteorder and compression bits" | closed 2018-02-02 | `ae76f45c` | ✅ |

**[#12014](https://github.com/openzfs/zfs/issues/12014)** — *"ZFS corruption
related to snapshots post-2.0.x upgrade"*, `Component: Encryption`, **296
comments**, closed 2025-05-19. The fix author (George Amanakis, upstream
contributor) states verbatim:

> "The corruption seen here affected **only non-raw sends**. It was fixed in
> c5228ba3481bbc9cdc88a06cf7afb4e6459ab9cd."

⚠️ **Citation-integrity note:** the SHA upstream cites is a *pre-rebase
PR-branch SHA* and exists in no release tag. The actually-merged commit is
`ea74cdedda8b`, first released in **zfs-2.4.0** — so **26.04's 2.4.1 has this
fix (PROVEN)**, while **24.04's 2.2.2 does not** (the 2.2.x backport landed in
2.2.8; graded INFERRED-strong: proven absent from upstream 2.2.2, no backport
found in noble's changelog, but noble's full patch series was not diffed).
*This is an argument for 26.04 over 24.04 independent of everything else in
this report.*

**🔴 STILL OPEN as of 2026-07-16** — none of these are fixed in 2.4.1:

| Issue | Title | Last activity |
|---|---|---|
| [**#12614**](https://github.com/openzfs/zfs/issues/12614) | "Replicating encrypted child dataset + **change-key** + incremental receive **overwrites master key of replica**, causes permission denied on remount" | **2026-02-15** |
| [**#12123**](https://github.com/openzfs/zfs/issues/12123) | "After replicating the encrypted dataset and perform key inheritance on the target (**change-key -i**), next incremental snapshot will break the dataset\volume" | 2024-02-10 |
| [#14330](https://github.com/openzfs/zfs/issues/14330) | "Another encryption bug: 'unencrypted block in encrypted object set'" | 2025-07-21 |
| [#14709](https://github.com/openzfs/zfs/issues/14709) | "ZFS **Kernel Panic** during encrypted raw send" | 2025-12-07 |
| [#13491](https://github.com/openzfs/zfs/issues/13491) | "Panic while receiving and encrypting dataset" | 2025-05-08 |
| [#12732](https://github.com/openzfs/zfs/issues/12732) | "PANIC at dmu_recv.c on receiving snapshot to encrypted file system" | 2025-05-28 |

**#12614 is precisely the `change-key` + raw-send bug the brief asked about,
it is OPEN, it has a self-contained reproducer, and it had activity five
months ago.** There are **39 open `Component: Encryption` issues** total.

**Two honest caveats, both required by the brief's standards:**

1. **The direct question went unanswered upstream.** On 2025-12-27 a user
   asked on #12014: *"I still find the issue rather confusing and want to ask
   if there are other corruption bugs affecting raw sends... that are
   lingering or this was the only one that was known?"* That is **comment
   296 of 296 — the last one — and no maintainer has answered it in ~7
   months.** Draw your own conclusion; I will not fill the silence.
2. **"Encrypted send/recv is not production ready" — COULD NOT VERIFY as an
   upstream position.** No maintainer declaration, no README/man warning, no
   tracking meta-issue says this. The defensible version is the evidence
   above, not the slogan.

**Excluded deliberately:** [#15526](https://github.com/openzfs/zfs/issues/15526)
(the famous Nov-2023 "files corrupted, chunks replaced by zeros" bug) is a
**block-cloning/`dnode_is_dirty`** bug, **not** an encryption bug. It
dominates searches for "OpenZFS corruption" and does not belong in this list.

### 🎯 The risk assessment that actually matters for U1

**The open bugs cluster entirely in replication, not at-rest encryption.**
Every open issue above requires `send -w`, `recv`, or `change-key -i`.

> **U1's rpool is a local root pool.** If we do not raw-send it and do not
> run `change-key` on a replica, **we touch none of these bugs.** Native
> encryption for a local root is in a materially different risk class from
> native encryption as a replication substrate.

This is the crux of the §3 evaluation, and it cuts both ways — see below.

---

## 2. Can clevis drive ZFS native encryption directly?

**Direct answer: No — and it does not need to.** The `clevis luks bind`
layer is structurally LUKS2-only, but the **generic `clevis encrypt` /
`clevis decrypt` JWE layer is genuinely LUKS-free** and will encrypt/decrypt
a ZFS key as an arbitrary blob today.

### The architectural split — this is the load-bearing fact

`clevis(1)`, verbatim:

> "Clevis is pluggable. Our plugins are called pins. The job of a pin is to
> take a policy as its first argument and **plaintext on standard input** and
> to encrypt the data so that it can be automatically decrypted if the policy
> is met."

The dispositive proof is the entire generic decrypt path
([`src/clevis-decrypt`](https://github.com/latchset/clevis/blob/master/src/clevis-decrypt)):

```bash
read -r -d . hdr
if ! pin="$(jose fmt -q "$hdr" -SyOg clevis -Og pin -Su-)"; then
    echo "JWE is missing the required 'clevis.pin' header property!" >&2
    exit 1
fi
if ! cmd="$(findexe clevis-decrypt-"$pin")"; then
    echo "Unable to locate pin '$pin'!" >&2
    exit 1
fi
(echo -n "$hdr."; /bin/cat) | "$cmd"
```

JWE on stdin → exec `clevis-decrypt-$pin` → plaintext on stdout. **Zero LUKS,
zero device, zero cryptsetup.** A ZFS key is just plaintext. PROVEN.

So a shim is trivially expressible:

```sh
clevis decrypt < /etc/keys/rpool.jwe | zfs load-key -L prompt rpool
```

This composes Finding 2's stdin path with clevis's generic layer. Both halves
are documented and neither is a hack.

### The complete pin list — correcting a common assumption

From [`src/pins/`](https://github.com/latchset/clevis/tree/master/src/pins):
**tang, tpm2, tpm1, sss, pkcs11, file**. Notable corrections to the brief's
assumed list:

- **`null` is not a standalone pin** — it is an sss-internal test helper.
- **`pkcs11` IS upstream now** (not third-party).
- **`file` exists** but its own man page says *"Rather for educational
  purposes."*
- **There is no FIDO2 pin.** PROVEN — no `src/pins/fido2`; issue
  [#55 "U2F Key Support?"](https://github.com/latchset/clevis/issues/55) was
  closed in 2018 without one; [PR #399](https://github.com/latchset/clevis/pull/399)
  was approved but never merged. A third-party
  [olastor/clevis-pin-fido2](https://github.com/olastor/clevis-pin-fido2)
  exists (13 stars, last commit 2024-11-01, no releases, README leads with
  "⚠️ **Use at own risk and consider this plugin to be experimental right
  now.** ⚠️"). **Not fleet-grade.** It would ride the generic layer
  automatically if installed on `$PATH`.

### `sss` can mix tang AND tpm2 in one threshold — PROVEN from the format spec

`clevis-encrypt-sss(1)` specifies `pins` as `{PIN:CFG,...}` or
`{PIN:[CFG,CFG,...],...}` with `t` = *"Number of pins required for decryption
(REQUIRED)"*. The object is keyed by pin name and `t` is pin-agnostic, so
"2 of {tang1,tang2,tang3,tpm2}" is directly expressible:

```json
{"t":2,"pins":{"tang":[{"url":"..."},{"url":"..."},{"url":"..."}],"tpm2":[{}]}}
```

`clevis(1)` confirms the intent: *"Clevis provides a way to **mix pins**
together to create sophisticated unlocking and high availability policies."*

⚠️ **Graded honestly: no verbatim upstream example of a *mixed* tang+tpm2 sss
config was found.** The claim rests on the format spec, which is unambiguous,
but per the brief's "don't tell me it works because it should work" standard
— **this specific composition is INFERRED, and is worth a VM test before we
rely on it.**

### Has anyone actually done clevis + native ZFS?

**Yes — and upstream clevis declined to support it** (both PROVEN):

- [Issue #218](https://github.com/latchset/clevis/issues/218) "Use clevis for
  ZFS native encryption passphrase" — opened 2020-08-02, **still open**.
- [PR #373](https://github.com/latchset/clevis/pull/373) "Add support for ZFS
  encryption" — opened 2022-05-24, **never merged**, stale since 2024. Its
  design stored the JWE in ZFS *user properties* prefixed `latchset.clevis`,
  split across 8K-limited properties. Abandoned.

**A real working implementation exists** (SECONDARY source, labeled):
[andrwe.gitlab.io — Proxmox with encrypted ZFS via Tang & Clevis](https://andrwe.gitlab.io/en/howto/proxmox-encrypted-zfs/)
uses ZFS native encryption with **no LUKS**:

```bash
zfs create -o encryption=aes-256-gcm -o keyformat=raw -o keylocation="file://${tmp_keypath}" rpool/home/user
clevis encrypt tang '{"url":"http://tang:8888", "adv": "..."}' < "${tmp_keypath}"
```

then a `clevis-decrypt@.service` decrypts the JWE into a **tmpfs** and
`zfs load-key -a` reads it. Clevis never touches ZFS — it just produces a
keyfile. **This is the shim, in production, by a third party.**

### Boot integration: clevis has no generic non-LUKS unlocker — PROVEN

`clevis-luks-unlockers(7)` lists four unlockers (`clevis-luks-unlock`,
dracut, systemd, udisks2) — **all LUKS-bound**. There is no arbitrary-secret
boot hook. **However**, the crucial detail (re-verified by hand this session
by reading `clevis-luks-askpass` in full):

```bash
while read -r line; do
    case "$line" in
        Id=cryptsetup:*) d="${line##Id=cryptsetup:}";;
        Socket=*) s="${line##Socket=}";;
    esac
done < "$question"

[ -e "${d}" ] || continue
[ -S "${s}" ] || continue

if ! pt="$(clevis_luks_unlock_device "${d}")" || [ -z "${pt}" ]; then
    continue
fi
```

**`clevis-luks-askpass` answers *any* `Id=cryptsetup:<device>` ask file,
reading the clevis token straight off that device's LUKS2 header. It does
not consult `/etc/crypttab` to decide what to unlock.** Crypttab appears only
in the loop-*termination* check (`clevis_devices_to_unlock`, which opens with
`[ ! -r /etc/crypttab ] && return 1`).

**This is the hinge of the entire recommended design** (§7): it means the
Ubuntu keystore's imperative `systemd-cryptsetup attach` — which creates no
crypttab entry — can still be answered by clevis/Tang.

---

## 3. The LUKS keystore idea — honest evaluation

The brief asked for an honest evaluation, including "does this defeat the
point of native encryption?" Here it is, and the answer is nuanced in a way
that matters.

### It is sound, and it is not our invention

Ubuntu's shipped design (Finding 3): ZFS `keylocation=file:///run/keystore/<pool>/system.key`,
where the key file lives on a **LUKS-encrypted zvol** at
`/dev/zvol/<pool>/keystore`, opened by `systemd-cryptsetup` before
`zfs load-key` runs. Because the keystore is **ordinary LUKS2**, the entire
clevis/Tang/`systemd-cryptenroll`/TPM2/FIDO2 toolchain applies to it
**unchanged**.

The original convention comes from `4000-zsys-support.patch` (Ubuntu's zsys
desktop ZFS installer); `4001` ports it to dracut. This is a maintained
distro path with an active maintainer (Benjamin Drung, Canonical), patched as
recently as **April 2026**.

### ⚠️ The honest part: what native encryption actually buys us *given a keystore*

**Unlock security becomes identical to ZFS-on-LUKS.** Both designs reduce to
"unlock a LUKS2 thing via Tang/TPM2/FIDO2, then use the key." The keystore
does not weaken native encryption — but it also does not make the *unlock*
story any better than what we already run today. Anyone who expects native
encryption to improve unlock security is mistaken.

So what is left? Three real wins and one that is specific to us:

**(a) Raw send (`send -w`) to untrusted targets.** The genuine data-path win:
back up rpool to a machine that cannot read it, with keys never loaded. This
is impossible with ZFS-on-LUKS (where ZFS sees plaintext and a "raw" send is
plaintext).
⚠️ **But this is exactly the feature with open correctness bugs (§1).** The
one thing native encryption uniquely buys is the one thing least safe to use.
That irony should be stated plainly rather than buried.

**(b) Per-dataset encryption roots.** Different keys for different datasets.
Not currently a requirement for U1.

**(c) 🔑 The one that is actually decisive here — ZFS self-healing requires
native mirroring.**

> ⚠️ **CORRECTED 2026-07-17.** This subsection originally argued that dropping
> IMSM forces ZFS-on-LUKS into **2** LUKS containers (2× enrollment surface)
> while native+keystore needs only 1. **That argument was wrong**, and the
> adversarial review
> ([design review §R7](2026-07-17-zfs-native-encryption-design-review.md))
> correctly demolished it. The original text is retained below the line for
> honesty about the error. The *conclusion* (native + keystore) survives; the
> *reasoning* does not.

**Why the original argument failed:** it had an unstated premise — that no md
layer may sit beneath LUKS. But **real Linux mdadm RAID1 is not IMSM
fakeraid.** An `mdadm` RAID1 across `sda3`+`sdb3` presents a single `/dev/md*`,
taking exactly **one** LUKS container — 1× enrollment, real mirroring, no
fakeraid. The locked decision bans *IMSM fakeraid*; it does not make 2
containers a technical necessity. And the "2× everything" cost model conflated
**scriptable** cost (2 Tang binds = one `for` loop, ≈0 marginal human effort)
with **interactive** cost (only a FIDO2 touch and a typed PIN genuinely
double).

**The correct argument — data integrity, not enrollment surface** (INFERRED
from ZFS's documented self-healing model): with md beneath LUKS, **ZFS sees one
vdev**. It can still *detect* corruption via checksum, but it **cannot repair
it** — there is no second copy to repair *from*, and md hands up a single block,
silently picking a half on mismatch. **Native ZFS mirroring is what makes scrub
self-healing.** That is the reason to choose it.

⚠️ **And the keystore is not "strictly better" either.** The claim below that a
ZFS-mirrored keystore makes the duplication objection "not apply" **claims too
much**. The keystore is **one logical LUKS2 header**; ZFS mirrors its *blocks*
faithfully — **including a logically-corrupt-but-checksum-valid write** (a torn
`luksAddKey`, a bad metadata update, an operator `luksKillSlot` mistake). Both
halves receive the same corruption, because ZFS is doing its job. ZFS protects
against **device death and bit-rot**, not **valid-but-wrong writes**. Two
independent LUKS containers carry two **independent headers**: a botched
enrollment on disk A leaves B openable. The keystore trades an
*independent-header* failure domain for a *device-death* one — a different
surface, not a dominant one. Mitigation (header backup) collides with §6's
revocation finding: an accepted, documented tension.

**The two decisive arguments are therefore: (i) ZFS self-healing requires
native mirroring, and (ii) Ubuntu ships and maintains the keystore path.** The
enrollment-surface framing is retired.

<details>
<summary>Original (wrong) argument, retained for the record</summary>

> We have already decided to drop IMSM in favour of native ZFS mirroring. Today,
> U1 has **one** LUKS container because IMSM presents **one** `/dev/md126`
> device to LUKS. Remove IMSM, and that stops being true:
>
> | Design (post-IMSM) | LUKS containers | Enrollment surface |
> |---|---|---|
> | ZFS-on-LUKS, mirrored | **2** — one per disk, both must unlock before import | **2×** everything |
> | **Native ZFS + keystore zvol** | **1** — the keystore, *inside* the mirrored pool | **1×** everything |
>
> **Dropping IMSM makes ZFS-on-LUKS strictly worse and native+keystore strictly
> better, simultaneously.** ... *(Attack this: an alternative is one LUKS
> container on one disk holding a keyfile that unlocks the second — but that is
> a hand-rolled single-point-of-failure...)*

**The error:** that parenthetical dismissed a *strawman*. The real alternative
was md-RAID1-under-LUKS, which was never considered.

</details>

### What it costs

- **Complexity**: a chicken-and-egg dance (import pool → find zvol → LUKS
  open → mount → load key → mount root). Ubuntu has already implemented and
  debugged it, which is most of the cost retired.
- **A LUKS2 header inside the pool.** Ordinary LUKS caveats apply — in
  particular, header backups defeat `luksKillSlot` revocation forever (§6).
- **Metadata leak** (accepted per the brief).
- **The dataset-layout question** — see "What I could not determine".

### Comparison against the alternatives

| Design | Unattended? | Enrollment surface | Distro support | Notes |
|---|---|---|---|---|
| **Native + LUKS keystore zvol** | ✅ Tang | **1× LUKS2** | ✅ **Ubuntu ships it** | Recommended |
| Native + clevis JWE shim (no LUKS) | ✅ Tang | JWE file; no keyslots | ❌ None (PR #373 dead) | Loses `cryptenroll` entirely: no FIDO2, no slot mgmt, no recovery keys |
| Native + `systemd-creds` (TPM2) | ⚠️ TPM2 only | n/a | Partial | Real, but no Tang and no multi-factor; seals against *current* PCRs |
| **ZFS-on-LUKS (status quo)** | ✅ Tang | **2× LUKS2** post-IMSM | ✅ Mature | No raw send; doubled enrollment once IMSM is gone |

The clevis-JWE-shim option deserves a specific note, because it is the one
the brief's §2 was gesturing at: it works (§2), but it **throws away
`systemd-cryptenroll` entirely**. No FIDO2, no `--wipe-slot`, no recovery
keys, no keyslot model — because those are all LUKS2 concepts. Given
`expect_fido2: true` is an asserted requirement in
`examples/configs/install/unimatrixone.yaml`, that is disqualifying **unless
FIDO2 is downgraded to on-site break-glass** (which §6 argues it must be
anyway — see the recommendation).

**Verdict: the keystore does not defeat the point of native encryption. It
does defeat the *unlock-security* argument for native encryption — but that
argument was never real. The keystore is the correct choice, for the reasons
in (c) plus distro support.**

---

## 4. TPM2 — can the key never leave the chip?

**The brief's suspicion is correct, and here it is proven rather than
asserted.** The honest answer is two-sided, and the split matters:

- **"Never leaves the chip" is REAL** for the TPM's *own* key operations — a
  `fixedTPM` + `sensitiveDataOrigin` key doing sign/decrypt/HMAC internally.
  No command outputs its private part.
- **"Never leaves the chip" is FALSE for a bulk disk key.** It comes back in
  cleartext via `TPM2_Unseal`, and the CPU does the AES work with the key
  resident in kernel memory.

### Sealing vs non-exportable objects

TCG Part 3 §12.7 (r1p59): *"This command returns the data in a loaded Sealed
Data Object."* Table 32 — `TPM2B_SENSITIVE_DATA outData`: "unsealed data /
**Size of outData is limited to be no more than 128 octets.**"
`tpm2_unseal(1)`: *"Returns a data blob in a loaded TPM object. **The data
blob is returned in clear.**"*

Also §12.7: if `restricted`, `decrypt`, or `sign` is SET, the TPM returns
`TPM_RC_ATTRIBUTES`. **A sealed blob is inert data, not an operational key** —
you cannot seal something *and* have the TPM use it. That closes the obvious
"just make it non-exportable" hope.

⚠️ **A trap most write-ups get wrong** — TCG Part 2, Table 31, bit 1, NOTE:

> "**fixedTPM does not indicate that key material resides on a single TPM**
> (see sensitiveDataOrigin)."

`fixedTPM` alone does **not** mean "generated on-chip, never existed
elsewhere" — it means "cannot be duplicated onward". You need
`sensitiveDataOrigin` too. The two are only trustworthy together.

### The core question, proven three ways

1. **The output is key-sized.** `outData` ≤ **128 octets**. Cannot stream disk
   blocks.
2. **The TPM *can* do AES — at ~1KB per round-trip.** Correcting the common
   overclaim: `TPM2_EncryptDecrypt` exists (Part 3 §15.2). But `inData` is
   capped at `MAX_DIGEST_BUFFER`, "required to be at least 1,024" bytes — every
   ~1KB is a full command round-trip over a slow serial bus.
3. **The TPM is not in the storage DMA path.** PROVEN: it attaches only via
   LPC/SPI/I2C (TCG PTP); PTP §7.4.1: *"**The TPM SHALL support an SPI clock
   frequency range of 10 - 24MHz.**"* It is a **slave**, not a bus master —
   PTP §8.1.4 requires the *host* bus master to support clock stretching so
   the TPM can throttle it. INFERRED: 24 MHz × 1-bit SPI ⇒ **~3 MB/s
   theoretical ceiling**, far lower in practice.

**Where the key actually lives** — PROVEN. kernel.org `dm-crypt.rst`: the key
is passed *"as `<key_string>` prefixed with single colon character (':') for
**keys residing in kernel keyring service**"*. Either way: kernel memory. ZFS
is equivalent.

**The contrast** — Apple Platform Security, verbatim:

> "dedicated AES-256 crypto engine (the AES Engine) **built into the direct
> memory access (DMA) path between the NAND (nonvolatile) flash storage and
> main system memory**"
> "**The AES Engine never exposes the unwrapped key to software.**"

That is the entire difference: Apple's engine is *inline*; the TPM is a
low-speed side peripheral. **The Apple Secure Enclave model is not
replicable on x86 + TPM2. DISPROVEN as wishful.**

**Aside — the real x86 "key never leaves" option is OPAL/SED.** TCG's SED
guide: *"the encryption key is never stored in the clear and **never leaves
the drive**."* **Does it help ZFS? No** (INFERRED): SED is block-level,
*beneath* the filesystem; ZFS native encryption sits above. They are
orthogonal, and SED does not survive the drive leaving the machine's auth
context. Not a path for us.

### PCR policy binding

PCR 0–7, verbatim from TCG PC Client Platform Firmware Profile v1.06 Rev 52,
Table 1 §3.3.4:

| PCR | Usage |
|---|---|
| 0 | SRTM, BIOS, Host Platform Extensions, Embedded Option ROMs and PI Drivers |
| 1 | Host Platform Configuration |
| 2 | UEFI driver and application Code |
| 3 | UEFI driver and application Configuration and Data |
| 4 | UEFI Boot Manager Code (usually the MBR) and Boot Attempts |
| 5 | Boot Manager Code Configuration and Data and GPT/Partition Table |
| 6 | Host Platform Manufacturer Specific |
| 7 | Secure Boot Policy |
| 8–15 | Defined for use by the Static OS |

⚠️ **PCR 0–7 do NOT measure the kernel/initramfs** — PROVEN (note 8–15 =
"Static OS"). With UKIs, systemd-stub measures the kernel into **PCR 11**.

**Why PCR 7 is conventionally chosen** (INFERRED from PROVEN): §3.3.4.8 shows
PCR[7] measures the `SecureBoot` variable, `PK`, `KEK`, `DB`, `DBX`, and —
§3.3.4.5(c) — *"the entry in the ...EFI_IMAGE_SECURITY_DATABASE that was used
to validate the UEFI image"*. It records **which certificate authorized the
binary, not the binary's hash** — so a kernel signed by the same key leaves
PCR[7] unchanged while PCR 0/2/4 all move.

What breaks bindings:

| Change | Breaks | Status |
|---|---|---|
| Firmware/BIOS update | PCR 0 | PROVEN |
| Boot order change | PCR 1 (`Boot####` + `BootOrder` measured here) | PROVEN |
| Kernel/initramfs update | PCR 4 / PCR 11 (UKI) — **not PCR 7** | PROVEN |
| Secure Boot key change (PK/KEK/db/dbx) | PCR 7 | PROVEN |
| Option ROM / GPU / pluggable HW | PCR 2 | PROVEN |

> ⚠️ **U1-specific**: our IPMI/BMC and the recent CMOS clear make PCR 0/1
> churn a live concern. `project_supermicro_unimatrixone_boot_hang` records a
> CMOS clear that already silently changed firmware settings once. A literal
> PCR binding on this host is fragile in a way it would not be on a quieter
> machine.

### 🚨 The default that silently changed — systemd 258

**Our config sets `tpm2_pcr_ids: "7"` explicitly** (`unimatrixone.yaml`),
which is good, **because the default is no longer 7.** systemd v258 NEWS,
verbatim:

> "systemd-cryptenroll, systemd-repart and systemd-creds no longer default to
> locking TPM2 enrollments to the current, literal value of PCR 7... **The new
> default PCR mask for new TPM2 enrollments is thus empty by default.**"

Ubuntu 26.04 ships **systemd 259.5-0ubuntu3** (PROVEN, Launchpad API) — well
past the change. **A plain `systemd-cryptenroll --tpm2-device=auto` on 26.04
binds to NO PCRs**, meaning the key unseals regardless of boot state.

⚠️ **Two distinct defaults — do not conflate** (both PROVEN):
- `--tpm2-pcrs=` (literal mask) → default **empty** (was `7` pre-258).
- `--tpm2-public-key-pcrs=` (signed policy) → default **11**, unchanged.

Upstream now recommends signed policies / `systemd-pcrlock` over literal PCRs
because "SecureBoot policy updates are typically managed by fwupd these days".
**But see the §5 blocker: that recommendation needs a UKI, which we do not
have.**

### PIN / authValue — and why it is worth more than it looks

TCG Part 1 §19.8: DA protection triggers on excessive auth failures →
`TPM_RC_LOCKOUT`. State variables: `failedTries`, `maxTries`, `recoveryTime`,
`lockoutRecovery`. Escape hatch: **`noDA`** — *"The authValue for an object
receives DA protection unless the object's noDA attribute is SET."*

The threat model is literally the TPM's design case — TCG Part 1 §13.7,
NOTE 1, verbatim:

> "**The primary attack model for the dictionary attack begins when a system
> falls into the hands of a thief. The thief tries to recover data on the
> system by guessing the password used to protect a disk's encryption keys.**"

**Why this matters for us:** a LUKS passphrase can be attacked offline at GPU
speed against a stolen header. **A TPM PIN cannot — every guess must go
through the chip, which counts.** So a low-entropy PIN is *viable*: entropy
isn't doing the work, hardware lockout is.

⚠️ **The corollary is the important half: PCR-only enrollment with no PIN is
the weak configuration.** An attacker who steals the whole machine just boots
it — PCRs reproduce, the TPM unseals, no secret needed. It protects against
*disk-pulled-from-machine*, **not machine theft**. And
`--tpm2-with-pin` **defaults to `no`**.

Other residual risks (INFERRED, well-supported): **bus sniffing** — a
discrete TPM's LPC/SPI traffic is unencrypted by default and the unsealed key
crosses that bus in the clear, probeable with cheap hardware (secondary
sources: SCRT, Pulse Security). U1 would use a **discrete LPC TPM** (§ below),
which is the sniffable kind. Mitigated by TPM session/parameter encryption,
which `systemd-cryptenroll` does and hand-rolled `tpm2_unseal` scripts
typically do not — a real argument for using systemd's path over DIY.

### What TPM2 hardware does the X10DSC+ actually have?

**The board supports TPM 2.0. Whether U1 has a module installed is
undeterminable without touching the hardware — which is out of scope.**

PROVEN from the X10DSC+ manual (**MNL-1805.pdf** — note: *not*
`MNL-X10DSC_.pdf`, that URL does not exist; retrieved via Wayback, as
Supermicro 403s automated fetches):

> `JTPM1    TPM (Trusted Platform Module)/Port 80 header`

The pin table proves it is **20-pin LPC** (`LCLK`, `LFRAME#`, `LAD0-3`,
`SERIRQ`, `CLKRUN#`, `LPCPD#`).

Part-number mapping, PROVEN verbatim from Supermicro's TPM User Guide table
"TPM Models and Supported AOMs":

| TPM 1.2 | TPM 2.0 |
|---|---|
| AOM-TPM-9655V / 9655H / -S / -C variants | AOM-TPM-9665V / 9665H / -S / -C variants |

Prose confirms: *"The TPM-9655 series uses TCG 1.2"* / *"The TPM-9665 series
uses TCG 2.0"*. **The brief's assumption (9655=1.2, 9665=2.0, 967x=2.0) is
confirmed** — with two caveats:

- ⚠️ **Anomaly:** Supermicro's own table lists `AOM-TPM-9665V-FS` / `9665H-FS`
  under **TPM 1.2** despite the 9665 number. Do not trust the "9665 ⇒ 2.0"
  heuristic for `-FS` parts.
- ⚠️ **AOM-TPM-9670 series is NOT compatible** — three disqualifiers: it is
  documented as **X11/LGA3647 only**; it is a **9-pin** header vs our 20-pin;
  and it is **SPI** vs our **LPC**. There is a trap here — the 9670 manual
  §1.3 contains boilerplate reading *"all X10 motherboards"*, which is generic
  "which boards have a TPM header" text, not a compatibility claim. A naive
  read buys the wrong module.
- **Compatible part: AOM-TPM-9665V**, whose product page states verbatim:
  *"**X10 motherboards with 20-pin TPM header**"*.

**X10DSC+ specifically supports TPM 2.0 — INFERRED (high confidence).** It
meets Supermicro's stated criterion exactly (X10 + 20-pin JTPM1, both proven),
but no retrieved source names "X10DSC+" by model in a TPM compatibility list.
Supermicro's TPM FAQ has no Wayback snapshot and 403s.

**BIOS minimum version for TPM 2.0 — COULD NOT VERIFY.** No Supermicro
statement found. One near-miss worth recording so nobody re-derives it: the
guide notes *"The TpmProvision command of SUM does not support TPM 2.0 on the
**Grantley** platform"* — Grantley **is** C612/E5-v3/v4, i.e. this board. But
that is a limitation of Supermicro's **SUM provisioning tool**, not of TPM 2.0
support. It implies TPM 2.0 exists on Grantley but cannot be TXT-provisioned
via SUM. Irrelevant unless we want TXT.

**Commands that would settle it, when hardware access is authorised:**

```bash
ls /sys/class/tpm/                                  # empty/absent => no TPM enumerated
cat /sys/class/tpm/tpm0/tpm_version_major           # 1 => TPM 1.2, 2 => TPM 2.0
dmesg | grep -i tpm                                 # tpm_tis (LPC) vs tpm_tis_spi
sudo tpm2_getcap properties-fixed | grep -A2 TPM2_PT_MANUFACTURER   # expect IFX (Infineon)
sudo dmidecode -t 43                                # SMBIOS TPM Device entry
```

> ⚠️ **This is a live risk to the plan.** `unimatrixone.yaml` asserts
> `enroll_tpm2: true` with `tpm2_pcr_ids: "7"`. If JTPM1 is **empty** — the
> header ships unpopulated and the TPM is a separately-purchased add-on —
> then TPM2 enrollment fails on this host and the design must not depend on
> it. **Nothing in our config or documentation records that a module was ever
> purchased or installed.** The `project_m715q_intel_amt` and related memories
> discuss TPM on *other* hosts. This must be checked before TPM2 is designed
> in as anything other than optional.

---

## 5. systemd key enrollment — where does it fit?

**Verdict: nothing in `systemd-cryptenroll` applies to ZFS. Several adjacent
systemd primitives do.**

### The negative, proven (not asserted)

Covered in Finding 1: `systemd-cryptenroll` is LUKS2-only by man page, by
architecture ("metadata... not available in other encryption formats"), and
by hard-coded `crypt_load(cd, CRYPT_LUKS2, NULL)`. There is no format
abstraction to extend, and **zero upstream issues even requesting it**.

What each enrollment type stores — all in the LUKS2 JSON token area plus a
keyslot:

| Option | What it stores |
|---|---|
| `--password` | "mostly equivalent to `cryptsetup luksAddKey`, however may be combined with `--wipe-slot=` in one call" |
| `--recovery-key` | "computer-generated instead of being chosen by a human... may be scanned off screen via a QR code" |
| `--pkcs11-token-uri=` | RSA: volume key encrypted to the token's public key. ECC: ECDH shared secret; "The generated private key is erased." |
| `--fido2-device=` | hmac-secret credential ID + salt |
| `--tpm2-device=` | TPM2-sealed randomized key |
| `--wipe-slot=` | Wipes by index or class (`all`, `empty`, `password`, `recovery`, `pkcs11`, `fido2`, `tpm2`) |

`--wipe-slot=` has a useful safety property, verbatim: *"the enrollment is
completed first, and only when successful the wipe operation executed — and
the newly added slot is always excluded from the wiping."* And: *"As safety
precaution an operation that wipes all slots without exception... is
refused."*

### What a ZFS design CAN reuse — the useful half

**(a) `systemd-creds` — format-agnostic, and the real TPM2 answer for ZFS.**
It seals *arbitrary bytes*; a ZFS key is arbitrary bytes. Nothing about it is
LUKS-aware. `--with-key=` takes `host`, `tpm2`, `host+tpm2`, `null`, `auto`,
`auto-initrd`. The critical line for initramfs use, verbatim:

> "When encrypting credentials that shall be used in the initrd (where
> `/var/lib/systemd/` is typically not available) make sure to use
> **`--with-key=auto-initrd`** mode, to disable binding against the host
> secret."

⚠️ **But a genuine gap: `systemd-creds` seals against *current* PCRs, and has
no pcrlock/signed-policy equivalent** — so it inherits the full
update-brittleness problem with none of the mitigation. (Corroborated by a
SECONDARY source — the NixOS write-up
[codgician.me, "A Secure ZFS Unlock Mechanism with TPM2"](https://codgician.me/en/posts/secure-zfs-auto-unlock-at-boot-nixos/)
— which independently reaches our conclusion, *"it only works with LUKS
volumes — not ZFS native encryption"*, builds on `systemd-creds`, and has to
hand-roll a `mkcreds` tool to pre-compute expected PCR values.)

**(b) `LoadCredentialEncrypted=` / `SetCredentialEncrypted=`** —
`zfs-load-key.service` with `LoadCredentialEncrypted=zfskey:...` then
`zfs load-key -L file://${CREDENTIALS_DIRECTORY}/zfskey` is a clean,
fully-supported composition. Plaintext lands in `$CREDENTIALS_DIRECTORY`,
backed by non-swappable memory where possible.

**(c) The ask-password protocol** — fully documented and reusable
([docs/PASSWORD_AGENTS.md](https://github.com/systemd/systemd/blob/v258/docs/PASSWORD_AGENTS.md)).
A querier drops an `.ini` at `/run/systemd/ask-password/ask.xxxx`; agents
watch via inotify and *"send a single datagram to the socket consisting of
the password string either prefixed with `+` or with `-`"*. **This is the
mechanism the recommended design runs on** (§7).

**(d) Direct `tpm2_unseal`** — always available, but you hand-roll policy,
PCR handling, and **session encryption (bus-sniffing protection)** that
`systemd-creds`/`cryptenroll` give free. Prefer (a) or the keystore.

### 🚨 `systemd-pcrlock` is experimental — and PCR 11 needs a UKI we don't have

`systemd-pcrlock(8)`, opening line, verbatim (identical v258 **and v259**):

> "**Note: this command is experimental for now.** While it is likely to
> become a regular component of systemd, it might still change in behaviour
> and interface."

It also *"requires a TPM2 device that implements the PolicyAuthorizeNV
command, i.e. implements TPM 2.0 version 1.38 or newer"* — an open question
for a hypothetical X10-era Infineon 9665.

And `systemd-pcrphase`, verbatim:

> "These services **require systemd-stub(7) to be used in a unified kernel
> image (UKI). They execute no operation when the stub has not been used to
> invoke the kernel.**"

⚠️ **Direct consequence: a standard Ubuntu shim+GRUB+vmlinuz boot is not a
UKI ⇒ no PCR 11 phase measurements ⇒ the signed-PCR-11 policy that upstream
now recommends does nothing for us.** Our options are literal PCRs (brittle,
and see the U1 CMOS-clear warning in §4) or experimental pcrlock. **This is a
real, unresolved constraint, not a detail.** It is the single largest
argument for treating TPM2 as a *convenience* rather than a load-bearing
unlock path on this host.

### crypttab / systemd-cryptsetup — options that matter to us

- **`headless=`** — verbatim: *"Takes a boolean argument, defaults to false.
  **If true, never query interactively for the password/PIN.** Useful for
  headless systems."* Without it, a failed token unlock **waits on a human
  forever** — a fail-open-to-hang behaviour we should set deliberately.
- ⚠️ **`initramfs` is NOT a systemd option** — PROVEN (`grep -c initramfs` on
  systemd's `crypttab.xml` = 0). It is Debian's, consumed by
  `cryptsetup-initramfs` hooks. **Our current code emits it**
  (`system_setup.rs:54`: `luks {} none luks,discard,initramfs`) — harmless
  under Debian's initramfs-tools, but meaningless to `systemd-cryptsetup`.
  Worth knowing when reading our own generated crypttab under dracut.
- `tries=` default 3; `0` = query indefinitely.
- `token-timeout=` defaults to **30s**, after which *"authentication via
  password is attempted"* — the automatic degradation path.

### clevis vs systemd-cryptenroll on the same volume — they coexist. PROVEN.

The brief asked not to use both without saying why it is safe. Here is why:
**the token `type` strings are disjoint**, verified in both source trees.

- systemd: `"systemd-tpm2"`, `"systemd-fido2"`, `"systemd-pkcs11"`,
  `"systemd-recovery"` — e.g.
  [`cryptsetup-token-systemd-tpm2.c#L16`](https://github.com/systemd/systemd/blob/v258/src/cryptsetup/cryptsetup-tokens/cryptsetup-token-systemd-tpm2.c#L16):
  `#define TOKEN_NAME "systemd-tpm2"`.
- clevis: `"clevis"` —
  [`clevis-luks-common-functions.in#L679`](https://github.com/latchset/clevis/blob/master/src/luks/clevis-luks-common-functions.in#L679):
  `printf '{"type":"clevis","keyslots":["%s"],"jwe":%s}'`

Each tool filters by its own type and ignores foreign tokens. **No token-type
collision is possible.**

They also do not fight over prompting: **clevis is a conforming password
agent, not a competitor.** PASSWORD_AGENTS.md: *"Multiple agents might be
running at the same time in which case they all should query the user and
**the agent which answers first wins**."* Coexistence is the designed
behaviour. They operate at different layers — clevis ships **no
libcryptsetup token module** (PROVEN: `grep crypt_token_register|token_open
clevis/src` → empty), so it unlocks via askpass while systemd uses its token
plugins.

Upstream context — [systemd#5182](https://github.com/systemd/systemd/issues/5182),
Poettering, verbatim:

> "We don't bother with reading LUKS superblocks ourselves. libcryptsetup does
> that for us. Hence: get your stuff supported by that, and we can support it
> too."

⚠️ Graded honestly:
- **Token IDs / keyslots — INFERRED, not proven.** Both allocate the next free
  ID via libcryptsetup, so in practice they differ; no quotable guarantee was
  found. Check `luksDump` before wiping slots.
- **A clevis+cryptenroll data-corruption failure mode — COULD NOT VERIFY.** No
  such bug report found. The realistic risk is **operator error during slot
  cleanup**, and **nondeterminism about which mechanism unlocked the volume**
  (permitted by "first wins", benign but debug-hostile). I will not manufacture
  a conflict the evidence does not show.

---

## 6. YubiKey / FIDO2

**Headline: a YubiKey cannot serve U1's unattended reboot, and this is a
spec-level fact, not a configuration problem.**

### The mechanisms

**FIDO2 `hmac-secret`** — CTAP2.1 §12.5, verbatim:

> "This extension is used by the platform to retrieve a symmetric secret from
> the authenticator... **The authenticator and the platform each only have the
> part of the complete secret to prevent offline attacks.**"
> "output1: **HMAC-SHA-256(CredRandom, salt1)**"

A subtlety worth recording: the authenticator holds **CredRandomWithUV** and
**CredRandomWithoutUV** — *"If uv bit is set to 1 in the response, let
CredRandom be CredRandomWithUV."* **Enrolling with UV off and later turning UV
on yields a different secret — the volume stops unlocking.**

**PIV/PKCS#11** — `systemd-cryptenroll(1)`, verbatim: *"For RSA, a randomly
generated volume key is encrypted with a public key in the token... To unlock
a volume, the stored encrypted volume key will be decrypted with a private key
in the token. For ECC, ECDH algorithm is used..."*

**OpenPGP** — not a native LUKS mechanism (no cryptenroll option), but *is*
reachable via clevis's `pkcs11` pin through OpenSC's PKCS#15 emulation.
Not relevant here.

### 🚨 FIDO2 user presence CANNOT be disabled — PROVEN four ways

This is the finding that settles §6 and much of §8:

1. **The spec** — CTAP2.1 §12.5, hmac-secret processing, verbatim:
   > **"If "up" is set to false, authenticator returns CTAP2_ERR_UNSUPPORTED_OPTION."**
2. **Yubico's libfido2 maintainer** ([libfido2#237](https://github.com/Yubico/libfido2/issues/237)):
   > "the CTAP2 spec also does not allow usage of the hmac-secret extension without user presence (UP)... **This extension is used by systemd.**"
3. **systemd silently overrides the flag.** Real output from that thread:
   > `👆 Locking without user presence test requested, but FIDO2 device /dev/hidraw1 requires it, enabling.`

   That string is in systemd's source (`src/shared/libfido2-util.c:1091`), with
   the comment: `/* If the token asks for "up" when we turn off, then this
   might be a feature that isn't optional. Let's enable it */`.
4. **The hardware** — Yubico: *"The YubiKey has a **capacitive touch sensor
   that cannot be controlled by software.**"*

⚠️ **Pre-empting a contradiction you will find if you spot-check this:** `up=false`
*is* legal in CTAP — for **pre-flight** (credential-existence probing), and
systemd does exactly that (`libfido2-util.c:366-369`). But a pre-flight
assertion **cannot return the hmac-secret**. Both facts are true and do not
conflict. `--fido2-with-user-presence=` is effectively inert for disk unlock;
systemd maintainers agree it should be removed
([systemd#23632](https://github.com/systemd/systemd/issues/23632), open since
2022).

**PIV cannot be unattended via systemd either — PROVEN, and for a
non-obvious reason.** Even with PIN-policy=never, Yubico's PKCS#11 module
hard-codes the login flag (`ykcs11.c:442`):

```c
slot->token_info.flags = CKF_RNG | CKF_LOGIN_REQUIRED | CKF_USER_PIN_INITIALIZED | CKF_TOKEN_INITIALIZED;
```

Unconditional (`=`, not `|=`). systemd only skips the PIN if that flag is
clear (`pkcs11-util.c:243`), else returns `-ENOANO`. **So systemd + YubiKey
PIV always prompts.** (Clevis's pkcs11 pin *can* be unattended by putting the
PIN in the config — `pkcs11:...?pin-value=the-pin` — with the obvious
consequences.)

### 🎯 The distinction that should drive our design

**A touch cannot traverse IPMI Serial-over-LAN; a PIN can.** That single
fact, not "FIDO2 is newer/simpler", is the right organising principle for a
headless racked server:

| Access model | Works | Does not |
|---|---|---|
| **Unattended** (power cut, 3am) | **Tang** only | **No token qualifies** |
| **Remote-interactive over SOL** | Typed recovery passphrase; PIV (plugged, touch=never, PIN typed) | **FIDO2 — you cannot tap a capacitive sensor over SOL** |
| **On-site physical** | FIDO2 touch; PIV | — |

systemd's own advice (`crypttab(5)`) is *"Typically the newer, simpler FIDO2
standard is preferable."* **That advice does not survive our constraints.**

### Multi-key enrollment and revocation

- **LUKS2 = 32 keyslots** (LUKS1 = 8). PROVEN — cryptsetup FAQ §3.4 + hard cap
  `LUKS2_KEYSLOTS_MAX` in `lib/luks2/luks2.h`.
- **FIDO2: N keys = N slots.** Structural — *"This master key is **unique per
  YubiKey**, generated by the device itself upon first startup, and **never
  leaves the YubiKey in any form**."* You cannot share a credential.
- **PIV: two YubiKeys CAN share one private key** — `ykman piv keys import`.
  **So N YubiKeys via PIV = 1 keyslot.** The opposite of FIDO2. (Caveat:
  imported keys can't be attested; generate air-gapped.)

**🚨 When a key is lost — `luksKillSlot` is NOT revocation.**
`cryptsetup-luksHeaderBackup(8)`, verbatim:

> "**The backup file and a passphrase valid at the time of backup allow
> decryption of the LUKS data area, even if the passphrase was later changed
> or removed from the LUKS device.**"

FAQ §6.7 confirms: *"if you change/disable a key-slot in LUKS, **a binary
backup of the partition will still have the old key-slot**."* Worse, FAQ §5.19
notes that **on SSDs the old keyslot may physically persist even without a
deliberate backup**. **Real revocation = `cryptsetup-reencrypt` (rotates the
volume key) + destroy every header backup.**

> This is a direct hit on our operational model: `luks_keys.rs` /
> `TASK-02-luks-rotate-revoke-guard` treat slot-kill as revocation. That is
> **not** sound against an attacker who ever had the header. Worth a
> follow-up review independent of this design.

⚠️ Also an automation hazard: `luksKillSlot` reading the passphrase from stdin
**silently enables batch mode and drops the last-keyslot guard**.

### FIDO2 in the initramfs — the Ubuntu trap

**Ubuntu's *default* root-unlock stack has ZERO FIDO2 support.** Verified
against shipped `.deb` contents: grepping every file in `cryptsetup-initramfs`
for `fido2|hidraw|libfido|tpm2|token-timeout|headless` → **no matches**.
Ubuntu's `crypttab(5)` is **Debian's, not systemd's**, and contains zero
`fido2`. Symptom:

> `cryptsetup: WARNING: nvme0n1p3_crypt: ignoring unknown option 'fido2-device'`

**This does not bite us** — precisely because we already use dracut (§7).
Recording it because it explains why most Ubuntu+YubiKey blog advice fails,
and it is a live reason *not* to fall back to initramfs-tools.

The dracut module is **`73fido2`** (dracut-ng main) / **`91fido2`** (Ubuntu's
dracut 106) — ⚠️ **not `95fido2`**. It installs libraries only; `hid_generic`/
`usbhid` come from the kernel-modules module and `60-fido-id.rules` from
udev-rules. A classic failure: `libfido2.so` is **dlopen'd**, so ldd-based
copying misses it → *"FIDO2 tokens not supported on this build."*
**Our `install_optional_items` line in `system_setup.rs:855` already copies
`libfido2.so*` explicitly — which is the correct instinct for exactly this
reason.**

### Does any FIDO2 path work with native ZFS?

**Yes, one tool exists — and it still cannot be unattended.**
[`fzifdso`](https://git.sr.ht/~nabijaczleweli/fzifdso) — on **sourcehut, not
GitHub**, which is why GitHub searches return empty. *"FIDO2/WebAuthn-based
(YubiKey, Somu, &c.) encryption keys for ZFS datasets."* Last commit
2025-05-03. It drives the documented raw+prompt+non-TTY-stdin channel from C
(`keyformat=raw` + `keylocation=prompt` as inert markers), i.e. exactly
Finding 2's shim. **But it never calls `fido_assert_set_up`, so UP defaults
true, and §12.5 binds it identically to systemd.** INFERRED (from omitted
`set_up` + spec).

⚠️ **A trap in the literature:** most "YubiKey + ZFS" blog results use the
**OTP slot's HMAC-SHA1 challenge-response** (`ykchalresp`) — a completely
different mechanism from FIDO2 hmac-secret. Do not confuse them.

### Model/firmware constraints — one that can waste money

**hmac-secret requires YubiKey firmware ≥ 5.2.x — NOT 5.0.** PROVEN from the
YubiKey 5 tech manual's "FIDO2 Extensions Available per Firmware Version"
table: `HMAC-Secret extension` is `yes` for 5.7.x–5.2.x and **blank for 5.1.x
and 5.0.x**. Firmware is immutable: *"Once programmed, YubiKeys cannot be
updated to another version."*

**A firmware 5.0/5.1 YubiKey does FIDO2 but CANNOT do FIDO2 disk unlock. Run
`ykman info` on every key before enrolling.**

| Model | FIDO2/hmac-secret | PIV |
|---|---|---|
| YubiKey 5 Series (fw ≥5.2) | Yes | **Yes** |
| Security Key Series | Yes | **NO** |
| YubiKey Bio (FIDO Edition) | Yes | **No** |
| YubiKey 4 | **No — U2F only** | Yes |

⚠️ **A Security Key Series cannot do the PIV/SOL path.** If we want PIV, it
must be a YubiKey 5.

---

## 7. dracut boot process

### 🎯 Settled: we are on dracut, and it is now Ubuntu's default

The brief said *"Ubuntu ships initramfs-tools by default, not dracut —
establish whether we're on dracut... Check the repo before assuming."*
Checked. **Three independent proofs, and the brief's premise is now
outdated:**

1. **The repo already uses dracut, deliberately.**
   `crates/uaa-core/src/network/ssh_installer/config.rs:16-22` makes
   `InitramfsType::Dracut` the `#[default]`, and
   `examples/configs/install/unimatrixone.yaml` sets `initramfs_type: dracut`
   explicitly. `system_setup.rs:365-379` installs `dracut dracut-network` +
   `zfs-dracut` for U1, with a comment: *"That install uses **dracut**, never
   initramfs-tools."*
2. **Ubuntu 26.04 made dracut the default** — PROVEN from resolute's
   `main/binary-amd64` Packages index. The generic kernel's dependency
   ordering has been **reversed**:
   ```
   Package: linux-image-7.0.0-14-generic
   Recommends: grub-pc | grub-efi-amd64 | ... , dracut | linux-initramfs-tool
   ```
   Historically this read `initramfs-tools | ...`. **dracut is now first ⇒
   fresh installs pull dracut.**
3. **The Foundations spec confirms the posture** —
   [discourse.ubuntu.com "Spec: Switch to dracut"](https://discourse.ubuntu.com/t/spec-switch-to-dracut/54776),
   by **bdrung** (the same Benjamin Drung who authored our keystore patch):
   > *"After some discussions and feedback, let's defer the demotion of
   > initramfs-tools to Ubuntu 26.10. Then **Ubuntu 26.04 LTS will support
   > both, dracut and initramfs-tools (with dracut being the default)**."*

⚠️ **Nuance the general advice misses: the switch is PER-FLAVOUR.**

| Flavour | First `Recommends` alternative | Default |
|---|---|---|
| `linux-image-*-generic` | `dracut \| linux-initramfs-tool` | **dracut** |
| `linux-image-*-azure` | `dracut \| ...` | **dracut** |
| `linux-image-*-realtime` / `-aws` / `-oracle` / `-ibm` | `initramfs-tools \| ...` | initramfs-tools |

Generic is our target. **Cost of switching: zero — already paid.**

🎯 **Corroborating fact that matters independently:** **`zfs-initramfs` is
absent from resolute `main/binary-amd64`**, replaced by **`zfs-dracut
2.4.1-1ubuntu5`** whose `Depends: dracut, systemd-cryptsetup, zfsutils-linux`.
**On 26.04, the ZFS+LUKS keystore path is a dracut path — there is no
initramfs-tools path in main.** (Scoped: only `main/binary-amd64` checked.)

⚠️ **Also: `dracut` `Conflicts: initramfs-tools`** — mutually exclusive, only
one can be installed. And a **debunk**: the widely-repeated blog claim that
`update-initramfs` is now a no-op shim is **FALSE** — dracut 110-11 ships a
**433-line functional reimplementation** (`# Taken from initramfs-tools and
adapted for dracut by Benjamin Drung.`) with real `-c/-u/-d/-k/-b/-v` handling.

**Ubuntu resolute ships dracut 110-11 (dracut-ng).** ⚠️ **Module numbers
changed in dracut-ng** and most documentation is stale:
`99base`→`80base`, `98dracut-systemd`→`77dracut-systemd`,
`95udev-rules`→`74udev-rules`, `90crypt`→`70crypt`, and `77initqueue` is split
out of base.

### Architecture

Modules live in `/usr/lib/dracut/modules.d/NNname/`. The `NN` prefix is a
**lexical sort order for module processing**, *not* a hook priority — hook
priority is the separate `<prio>` argument to `inst_hook`.

`check()` return codes, verbatim from `dracut.modules.7`:

> **0**:: Include the dracut module in the initramfs.
> **1**:: Do not include... requirements are not fulfilled.
> **255**:: Only include the dracut module, **if another module requires it**
> or if explicitly specified in the config file or on the argument list.

**That `255` is not trivia — it is the reason Ubuntu's April 2026 patch
exists.** See below.

`dracut.conf.d` fragments are read from `/run/initramfs/dracut.conf.d`,
`/usr/lib/dracut/dracut.conf.d`, `/etc/dracut.conf.d`, in alphanumeric order;
**must** end in `.conf` (other extensions silently ignored).

### ⚠️ FINDING — `hostonly` is NOT enabled on Ubuntu, and the man page is wrong

`dracut.conf.5` claims `hostonly` "(default=yes)". **The code disagrees.**
`dracut.sh:1410-1411` is a never-unset guard, not a default:

```sh
# make sure these variables are never unset
hostonly=${hostonly-}
```

There is no build-time default; Ubuntu's `/etc/dracut.conf` sets nothing, and
its only auto-loaded fragment (`01-debian.conf`) sets only `i18n_vars` and
`initrdname`. **A stock resolute initramfs is generic, not hostonly.** This
has large consequences below.

⚠️ **A loaded gun worth knowing about:** Ubuntu ships conf.d *subdirectories*
(`hostonly`, `no-network`, `generic`, `fips`...) which do **not** match the
`*.conf` glob and are opt-in via `--add-confdir`. One of them is:

```
# /usr/lib/dracut/dracut.conf.d/no-network/10-no-network.conf
omit_dracutmodules+=" net-lib systemd-networkd "
```

**`--add-confdir no-network` silently kills Tang.** Not on by default.

### The hook sequence

`dracut.bootup(7)`, verbatim (systemd mode — which Ubuntu uses):

```
                                    systemd-journal.socket
                                               |
                                               v
                                    dracut-cmdline.service
                                               |
                                               v
                                    dracut-pre-udev.service
                                               |
                                               v
                                     systemd-udevd.service
                                               |
                                               v
local-fs-pre.target                dracut-pre-trigger.service
         |                                     |
         v                                     v
 (various mounts)  (various swap  systemd-udev-trigger.service
         |           devices...)               |
         v               v                     v
  local-fs.target   swap.target     dracut-initqueue.service
         |               |                     |
         \_______________|____________________ |
                                              \|/
                                               v
                                        sysinit.target
```

⚠️ Two cmdline corrections that will otherwise cost debugging time:

- **`rd.zfs.force` does not exist.** `grep -rn "rd\.zfs\.force" zfs/` → zero
  matches. The real args are `zfs_force`, `zfs.force`, `zfsforce`.
- **The initqueue timeout knob is `rd.retry`, not `rd.timeout`.** Different
  things.

### 🎯 The chain that makes the recommended design work

This is the most important sequence in the report. Every link was traced in
source; the askpass link was **re-verified by hand this session**.

1. Ubuntu's 4001 patch changes `90zfs`'s `depends()` to
   `echo systemd-cryptsetup udev-rules` → **`71systemd-cryptsetup` is pulled
   in.**
2. **Why that was needed (LP #2148282)** — `71systemd-cryptsetup/module-setup.sh`:
   ```sh
   check() {
       require_binaries "$systemdutildir"/systemd-cryptsetup || return 1
       [[ $hostonly ]] || [[ $mount_needs ]] && {
           for fs in "${host_fs_types[@]}"; do
               [[ $fs == "crypto_LUKS" ]] && return 0
           done
           return 255
       }
       return 0
   }
   ```
   **The keystore's `crypto_LUKS` lives inside a zvol — not a host filesystem
   dracut can see at build time.** So it never lands in `host_fs_types`,
   `check()` returns 255, nothing requires it, and `systemd-cryptsetup` is
   absent → the patch's `systemd-cryptsetup attach` would be
   command-not-found. Adding it to `depends()` forces inclusion. The binary
   package agrees: `Package: zfs-dracut / Depends: dracut, systemd-cryptsetup, ...`
3. **Second-order effect that matters more than the first** —
   `71systemd-cryptsetup` also installs
   `"$systemdsystemunitdir"/sysinit.target.wants/cryptsetup.target`, so
   **`cryptsetup.target` is actually reached in the initrd**, as part of
   `sysinit.target`. *Ubuntu did not do this for clevis's benefit, but it is
   precisely the hook clevis needs.*
4. clevis's `systemctl add-wants cryptsetup.target clevis-luks-askpass.path`
   → the `.path` watcher (`DirectoryNotEmpty=/run/systemd/ask-password`)
   **starts there and stays armed**.
5. `dracut-pre-mount.service` is `After=basic.target dracut-initqueue.service
   cryptsetup.target` → `zfs-load-key.sh` (hook `pre-mount/90`) runs
   **strictly after the watcher is armed**.
6. `zfs-load-key.sh` → `systemd-cryptsetup attach keystore-rpool
   /dev/zvol/rpool/keystore` → **imperative, not via crypttab** → with no key
   file it falls back to interactive acquisition → posts
   `Id=cryptsetup:/dev/zvol/rpool/keystore` → `.path` fires → **clevis answers
   from Tang**.

**The early-exit in clevis's askpass loop (empty crypttab →
`remaining_crypttab` empty → `break`) is NOT fatal** — the `.path` unit
re-arms and restarts the service when a new ask file lands. This is subtle and
worth remembering: it is why "no crypttab entry" does not break the design.

**Ordering vs pool import is sound and triply-enforced** (the zvol cannot
exist before import): `zfs-load-key.sh` is `pre-mount/90`;
`dracut-pre-mount.service` is forced `After=zfs-import.target` by
`dracut-zfs-generator`; and the script *additionally* spin-waits on
`systemctl is-active --quiet zfs-import.target`.

### 🚨 THE HEADLINE GAP — Tang gets no network automatically, for TWO reasons

`clevis-pin-tang/module-setup.sh.in`:

```sh
install() {
    if [ "${hostonly_cmdline}" = "yes" ] && have_tang_bindings; then
        echo "rd.neednet=1" > "${initdir}/etc/cmdline.d/99clevis-pin-tang.conf"
        if dracut_module_included "systemd"; then
            mkdir -p "${initdir}/${systemdsystemunitdir}/clevis-luks-askpass.path.d"
            cat > "${initdir}/${systemdsystemunitdir}/clevis-luks-askpass.path.d/network-online.conf" <<EOF
[Unit]
After=network-online.target
Wants=network-online.target
EOF
        fi
    fi
```

**Everything that makes Tang work over the network is inside that `if`. Both
conjuncts fail on our target.**

- **Reason 1 (applies to BOTH architectures): `hostonly_cmdline` is not "yes"
  on Ubuntu.** It is only auto-enabled when `hostonly` is set — which the
  finding above proves is empty on a stock resolute build. So the entire block
  is skipped and **neither `rd.neednet=1` nor the `network-online.conf`
  ordering drop-in is ever written.**
- **Reason 2 (native+keystore ONLY): `have_tang_bindings` cannot see the
  keystore.** It iterates `clevis_devices_to_unlock`, which is crypttab-driven
  (`[ ! -r /etc/crypttab ] && return 1`). The keystore port **never creates a
  crypttab entry** → the zvol is invisible → returns 1.

✅ **We already work around Reason 1 — by accident of good instinct.**
`system_setup.rs:586-595` sets `GRUB_CMDLINE_LINUX` with `rd.neednet=1
ip=dhcp` for dracut+Tang, with the comment *"GRUB must pass rd.neednet=1
ip=dhcp so the network is available in the initramfs before Tang is
queried."* That is exactly the required compensation. Likewise
`build_dracut_crypt_conf` force-adds the NIC driver
(`system_setup.rs:839-843`) because hostonly pruning would omit it.

⚠️ **We do NOT work around the ordering half.** Because the
`network-online.conf` drop-in is never generated, `clevis-luks-askpass` is
**not ordered after `network-online.target`** — so clevis's Tang `curl` may be
attempted before the link is up. Today this is presumably masked by retry; it
is a latent race, not a proven failure. **Mitigation: ship the drop-in
ourselves.** (INFERRED — code-proven, not boot-tested.)

> **Scoping note, to prevent an over-broad conclusion:** clevis/Tang is **not
> "broken for root-on-ZFS."** What is broken is the *automatic network
> wiring* (both architectures, via Reason 1) plus *automatic binding
> discovery* (native+keystore only, Reason 2). Both are compensable, and we
> already compensate the first.

### Debugging

`rd.debug`, `rd.break=pre-mount` (drops a shell exactly where our unlock
runs), `rd.udev.log_level`, and `lsinitrd` to inspect a built image. For this
design, `rd.break=pre-mount` is the single most useful lever: it lands you
after pool import and before key load.

---

## 8. Unattended reboot — the constraint that decides everything

**U1 must come back from a power cut with nobody present.** Per-method,
plainly:

| Method | Survives unattended reboot? | Evidence |
|---|---|---|
| **Tang (clevis, network)** | ✅ **YES** | Non-interactive HTTP. The only qualifying method. |
| **TPM2, no PIN** | ⚠️ Technically yes | But: unseals for *anyone who boots the box* — machine theft defeats it (§4). And on 26.04 the default binds **no PCRs** unless we set them (we do). |
| **TPM2 + PIN** | ❌ **NO** | A human must type the PIN. Our config sets `tpm2_pin`. |
| **FIDO2 / YubiKey** | ❌ **NO — impossible** | CTAP2 §12.5: `up=false` ⇒ `CTAP2_ERR_UNSUPPORTED_OPTION`. Not configurable. |
| **PIV via systemd** | ❌ NO | `CKF_LOGIN_REQUIRED` hard-coded (§6). |
| **Recovery passphrase** | ❌ NO | Human at SOL. |

**Conclusion: Tang is the primary and only unattended path, in every
candidate design.** This does not discriminate between the architectures —
both the keystore and the JWE-shim can do Tang — so it does not decide the
§3 fork, but it does decide the *role* of every other method: **everything
except Tang is break-glass.**

Our current Tang posture (`unimatrixone.yaml`): 3 Tang servers
(172.16.2.45/46/47) with `tang_threshold: 2` — clevis `sss` 2-of-3. That
tolerates **one** Tang server down, unattended.

### Break-glass when Tang quorum is down

With 2-of-3, losing **two** RPis makes the host unbootable unattended. The
fallback ladder, in the order it would actually be used:

1. ~~**TPM2 + PIN over IPMI SOL.**~~ 🔴 **REFUTED — DO NOT ENROLL THIS.** See
   the corrections banner. A `systemd-tpm2` PIN token on the keystore is **not
   a fallback rung — it is a boot-hang** that pre-empts Tang entirely and hangs
   with no deadline. Enrolling it to gain a break-glass path would *destroy*
   the primary unattended path. (Independently, §5's UKI blocker already made
   the PCR policy here either brittle or experimental — so this rung was weak
   even before it was refuted.)
2. **Recovery passphrase over IPMI SOL.** `--recovery-key` gives a
   high-entropy computer-generated key. **This is the real backstop and it
   must exist.** ⚠️ Practical caveat: the prompt must actually *reach* the
   serial console (`console=ttyS0,115200`; plymouth must not swallow it) —
   [clevis#248](https://github.com/latchset/clevis/issues/248) is exactly
   plymouth breaking the unlock flow, workaround *"disable the splash screen
   (plymouth)"*. **`token-timeout=` defaults to 30s**, after which
   *"authentication via password is attempted"* — that is the automatic
   degradation path to this rung.
3. **YubiKey, on-site, physically.** Cannot help remotely (no touch over SOL).

⚠️ **A fail-open worth naming now rather than in the design review:** the
degradation path *is itself an attack surface*. As
[oddlama's analysis](https://oddlama.org/blog/bypassing-disk-encryption-with-tpm2-unlock/)
shows, *"the initrd will fall back to a password prompt, if TPM unlocking
fails for whatever reason"* — and auto-unlock *"does not automatically ensure
that the data on them is authentic."* An attacker who can force Tang to fail
(unplug the network) gets a password prompt instead of a locked box. That is
correct availability behaviour and a security cost simultaneously; `headless=`
(§5) is the lever that changes it.

---

## What I could not determine

Honest gaps, per the brief's explicit request. These are recorded as
undetermined rather than filled with plausible guesses.

1. **🔴 Whether U1 has a TPM module physically installed.** Undeterminable from
   documentation — the JTPM1 header ships **empty**; the TPM is a
   separately-purchased add-on. Supermicro's own test: *"If the board does not
   have this connector, then it does not support the TPM"* — header presence
   proves *capability*, never *population*. Nothing in our repo or memories
   records a module being purchased or installed. **`enroll_tpm2: true` in
   `unimatrixone.yaml` may be asserting something that cannot happen.** Needs
   the §4 commands, on hardware, when authorised. **Highest-value gap.**
2. ~~**The keystore dataset layout / a possible Ubuntu bug.**~~
   ✅ **RESOLVED 2026-07-17 — see the corrections banner above.** Unencrypted
   children of encrypted parents are **legal** (OpenZFS `179374cc`); Ubuntu's
   curtin creates `rpool/keystore` with `encryption=off` and sets encryption
   via `-O` on `zpool create`, so **the encryption root is the bare pool
   `rpool`** and the dracut patch is **correct for the stock layout**. No
   circularity — the layers are stacked, not nested. The `$ENCRYPTIONROOT`
   asymmetry *is* a real robustness bug, but it bites only **non-stock**
   layouts (encryption root below the pool root); it does not affect us if we
   use the stock layout, which we should. **The real bug in the dracut port is
   a zvol race**, requiring a wait hook we ship ourselves.
3. **BIOS minimum version for TPM 2.0 on X10DSC+.** No Supermicro statement
   found. The "Grantley/SUM TpmProvision" note is about *provisioning tooling*,
   not support.
4. **Whether OpenZFS #11983** ("Input/Output Error when sending an encrypted
   incremental dataset back to its source") was fixed by PR #12981. Closed
   2022-02-04 with no closing commit in the timeline. Plausibly the same fix;
   not proven.
5. **A verbatim upstream example of a mixed tang+tpm2 `sss` config.** The
   format spec makes it unambiguous, but no worked example was found. **Test in
   a VM before relying on it.**
6. **Whether clevis and systemd-cryptenroll can collide on token IDs/keyslots.**
   Both use libcryptsetup's next-free allocation; no quotable guarantee found.
   No corruption bug report exists either (also a gap, not a clearance).
7. **Whether the `#12014` fix was cherry-picked into noble's 2.2.2.** Proven
   absent upstream at 2.2.2 (fix landed 2.2.8); noble's full patch series was
   not diffed. Moot if we stay on 26.04.
8. **Per-vendor TPM `maxTries`/`recoveryTime` defaults.** The TCG spec defers to
   platform-specific specs. Read on-host via `tpm2_getcap properties-variable`.
9. **Whether a FIFO works as `keylocation=file://`.** The absence of an
   `S_ISREG` guard in `get_key_material_file()` is proven by source; that a
   FIFO therefore works is inferred, not executed. Note the sharp edge from the
   same code: **a blocking `fopen` on a FIFO with no writer hangs forever — no
   timeout in that path.** Not needed by the recommended design.
10. **Whether Ubuntu's `zfs-dracut` diverges from upstream's `90zfs`** beyond
    the patches inspected. The patch series was read; the built package
    contents were not diffed against it.

**Not verified by execution, anywhere in this report:** nothing here was
boot-tested. Every claim is source-, spec-, or package-metadata-derived. The
chain in §7 is *code-proven and not boot-proven* — per
`feedback_verify_the_test_before_trusting_the_result`, that distinction is
the whole point, and a VM test (not U1) is the correct next gate.

---

## Sources

Primary sources, pinned where possible. Full URLs inline above.

**OpenZFS** — tag `zfs-2.4.1` (the exact upstream base Ubuntu 26.04 ships):
`lib/libzfs/libzfs_crypto.c`, `man/man7/zfsprops.7`, `man/man8/zfs-load-key.8`,
`man/man8/zfs-send.8`, `contrib/dracut/90zfs/`. Issues #6845, #10523, #11983,
#12014, #12123, #12594, #12614, #12732, #13491, #14330, #14709, #15526; PRs
#12981, #14704; commit `6e015933`, `ea74cdedda8b`.

**Ubuntu** — `git.launchpad.net/ubuntu/+source/zfs-linux` branch
`ubuntu/resolute` (`zfs-linux 2.4.1-1ubuntu5`), `debian/patches/ubuntu/4000-zsys-support.patch`,
`4001-dracut-Open-and-mount-luks-keystore.patch`, `debian/changelog`,
`debian/control`; `archive.ubuntu.com/ubuntu/dists/resolute/main/binary-amd64/Packages.xz`;
Launchpad API (`api.launchpad.net/1.0/ubuntu/...`); LP bugs #2070066, #2148282;
[Foundations spec: Switch to dracut](https://discourse.ubuntu.com/t/spec-switch-to-dracut/54776).

**clevis** — `latchset/clevis`: `src/clevis-decrypt`, `src/luks/clevis-luks-bind`,
`src/luks/systemd/clevis-luks-askpass.in` + `.path` + `.service.in`,
`src/luks/dracut/clevis/module-setup.sh.in`,
`src/luks/dracut/clevis-pin-tang/module-setup.sh.in`,
`src/luks/clevis-luks-common-functions.in`, `src/pins/`. Issues #55, #218, #248;
PRs #373, #399.

**systemd** — tags `v258`/`v259`: `man/systemd-cryptenroll.xml`, `man/crypttab.xml`,
`man/systemd-creds.xml`, `man/systemd-pcrlock.xml`, `man/systemd-pcrphase.service.xml`,
`docs/PASSWORD_AGENTS.md`, `NEWS`; `src/cryptenroll/cryptenroll.c`,
`src/cryptsetup/cryptsetup.c`, `src/cryptsetup/cryptsetup-tokens/`,
`src/shared/libfido2-util.c`, `src/shared/pkcs11-util.c`. Issues #5182, #23632,
#24940, #32586, #35443.
⚠️ **Method note:** `freedesktop.org/software/systemd/man/latest/*` returns
**HTTP 403** to automated fetches. Use `raw.githubusercontent.com/systemd/systemd/<tag>/man/*.xml`.

**dracut** — `dracut-ng/dracut-ng` (Ubuntu ships **dracut-ng 110-11**):
`man/dracut.modules.7.adoc`, `man/dracut.conf.5.adoc`, `man/dracut.bootup.7.adoc`,
`man/dracut.cmdline.7.adoc`, `dracut.sh`, `modules.d/71systemd-cryptsetup/`,
`modules.d/70crypt/`.

**TCG** — TPM 2.0 Library r1p59 Parts 1–3; PC Client Platform TPM Profile v1.04;
PC Client Platform Firmware Profile v1.06 Rev 52; Architects Guide to SEDs.
⚠️ Supermicro PDFs 403 automated fetches — retrieved via Wayback Machine.

**Vendor** — Supermicro X10DSC+ manual **MNL-1805.pdf** (⚠️ *not*
`MNL-X10DSC_.pdf` — that URL does not exist), Supermicro TPM User Guide Rev 1.20,
AOM-TPM-9670V/9670H manual Rev 1.2a, AOM-TPM-9665V product page; Yubico
YubiKey 5 tech manual, PIV Introduction/Yubico extensions, libfido2 issue #237,
`ykcs11.c`; Apple Platform Security guide; cryptsetup FAQ + LUKS2 on-disk format spec;
CTAP 2.1 (FIDO Alliance); kernel.org `dm-crypt.rst`.

**Secondary (labeled as such; no claim rests on these alone)** —
[andrwe.gitlab.io Proxmox+ZFS+Tang](https://andrwe.gitlab.io/en/howto/proxmox-encrypted-zfs/),
[codgician.me ZFS+TPM2 on NixOS](https://codgician.me/en/posts/secure-zfs-auto-unlock-at-boot-nixos/),
[oddlama on TPM2 unlock bypass](https://oddlama.org/blog/bypassing-disk-encryption-with-tpm2-unlock/),
[fzifdso](https://git.sr.ht/~nabijaczleweli/fzifdso), SCRT/Pulse Security TPM
bus-sniffing write-ups.

**Repo** — `crates/uaa-core/src/network/ssh_installer/config.rs:16-22`,
`system_setup.rs:54,365-379,519-522,586-595,818-857`,
`examples/configs/install/unimatrixone.yaml`.
