<!-- file: docs/agent-tasks/2026-07-16-zfs-native-encryption-research-prompt.md -->
<!-- version: 1.0.0 -->
<!-- guid: fa973275-bfba-4221-9ab6-fea0347bb788 -->
<!-- last-edited: 2026-07-16 -->

# Research prompt: ZFS native encryption + unlock architecture

Paste the block below into a fresh session. It is self-contained.

---

Research task, then design. Do the research FIRST and write a report before any
design work — I want to see the evidence, not a conclusion.

## Context you need

This repo (`ubuntu-autoinstall-agent`) installs Ubuntu with an encrypted ZFS
root. The **current** design is ZFS-on-LUKS: `bpool` (unencrypted ZFS boot
pool) + LUKS container holding `rpool`, unlocked via Tang/clevis, TPM2+PIN,
and YubiKey/FIDO2. Target host is `unimatrixone` (U1), a Supermicro X10DSC+
with 2x931GB SATA disks and a 13.4GB Intel Optane NVMe.

**Decision already made (do not relitigate):** we are dropping Intel IMSM
fakeraid in favor of native ZFS mirroring. mdadm's only possible remaining
role is the ESP. See the `project_storage_arch_drop_imsm_native_zfs` memory.

**My stated preference:** I want **native ZFS encryption**, and I accept that
it leaks metadata (dataset/folder names, sizes, snapshot structure). Don't
talk me out of it on metadata-leak grounds alone — but DO tell me if it breaks
something functional I haven't considered.

## Research questions

Answer each with real evidence — cite upstream docs, man pages, source, and
mailing-list/issue threads. Where something is version-dependent, say which
version. Where you can't find a definitive answer, say so explicitly rather
than guessing. Prefer primary sources over blog posts.

### 1. Native ZFS encryption — how it actually works
- Mechanics: encryption roots, key hierarchy, wrapping keys vs master keys,
  `keyformat` (raw/hex/passphrase), `keylocation` (prompt/file/https), what
  `zfs load-key` / `zfs change-key` actually do.
- What IS and IS NOT encrypted. Be precise about the metadata leak.
- How key prompting works at boot, and what drives it.
- Inheritance: encryption roots vs child datasets, and what happens on
  send/recv (raw sends, `-w`).
- Known sharp edges — I've heard there are correctness bugs around
  raw send/recv and `zfs change-key`. Find out if they're fixed and in which
  OpenZFS version.

### 2. Can clevis drive ZFS native encryption directly?
This is the crux. Clevis is built around LUKS (`clevis luks bind`). Determine
whether clevis can bind to ZFS native encryption AT ALL, or whether it
fundamentally can't and needs a shim. If a shim: what shape? A `keylocation`
pointing at a script? Does ZFS support that?

### 3. The LUKS keystore idea (my original thought — evaluate it honestly)
A small LUKS-encrypted partition holding the ZFS encryption key, so the whole
existing clevis/Tang/TPM2/FIDO2 toolchain still applies, and ZFS just reads
the key file from the unlocked keystore. Is this sound? What does it cost?
Does it defeat the point of native encryption? Compare against alternatives.

### 4. TPM2 — can the key never leave the chip? (Apple Secure Enclave model)
I want to know if this is real or wishful. Research:
- TPM2 sealing vs key objects that are non-exportable.
- Can a TPM2 perform the decryption itself so the key never enters host RAM,
  the way Apple's Secure Enclave works? Or does the TPM inevitably release the
  key to the kernel for ZFS/dm-crypt to use? **Be honest — I suspect the
  answer is "no, it gets released," but I want it proven, not assumed.**
- PCR policy binding, what breaks PCR bindings (firmware updates, boot order
  changes), and PIN/`authValue` protection.
- What TPM2 hardware/firmware this specific Supermicro X10DSC+ actually has.

### 5. systemd key enrollment — where does it fit?
- `systemd-cryptenroll`: TPM2, FIDO2, PKCS#11, recovery keys. What it does and
  what it only does for LUKS.
- `systemd-cryptsetup`, crypttab options, `systemd-pcrlock`/`systemd-pcrphase`.
- Does any of it apply to ZFS native encryption, or is it LUKS-only?
- How it overlaps or conflicts with clevis — do NOT use both on one volume
  without saying why that's safe.

### 6. YubiKey / FIDO2 authentication
- FIDO2 `hmac-secret` vs PIV/PKCS#11 vs OpenPGP — which is right here and why.
- Which need a physical touch, which need a PIN, which work headless/remote.
- How a YubiKey unlock interacts with an unattended reboot (it can't — so what
  is the actual fallback story?).
- Multi-key enrollment and revocation. What happens when a key is lost.

### 7. dracut boot process — go deep
- Full flow: module discovery, hooks (cmdline/pre-udev/pre-mount/mount/
  pre-pivot), `initqueue`, and where each unlock method injects itself.
- How `zfs-dracut` works, what it generates, how it finds and imports pools.
- How clevis-dracut and the systemd-cryptsetup dracut modules hook in.
- Network-in-initramfs for Tang (`rd.neednet`, ip= cmdline), and ordering vs
  pool import.
- **Ubuntu ships initramfs-tools by default, not dracut** — establish whether
  we're on dracut or initramfs-tools here, what this repo currently uses, and
  what switching would cost. Check the repo before assuming.

### 8. Unattended reboot — the requirement that constrains everything
U1 must come back from a power cut with no human present. Tang (network-bound)
is the primary path today. For each candidate design, state plainly whether it
survives an unattended reboot, and what the break-glass path is when the Tang
server is down.

## Deliverables

1. **A research report** in `docs/` following this repo's conventions (read
   `docs/process/status-reports.md` and `docs/process/executive-summaries.md`
   first — do not improvise the structure; file headers are mandatory per
   `.standards/instructions/file-headers.md`). Evidence and citations, clearly
   separating what is proven from what is inferred. Include a "what I could not
   determine" section — I would rather have honest gaps than confident guesses.

2. **Then** dispatch an authentication/security specialist agent to review the
   report adversarially and design the actual solution. Give it the full report
   plus the unattended-reboot constraint and my stated preference for native ZFS
   encryption. I want a concrete design: partition layout, key hierarchy, each
   enrollment path, the boot sequence, failure modes, and the break-glass
   procedure. Have it explicitly attack its own design for fail-open behavior.

3. **A recommendation from you** at the end, in your own words: what you'd
   actually build and why, including where you disagree with the specialist if
   you do.

## How I want you to work

- Research before designing. Don't jump to a conclusion and backfill.
- Separate evidence from hypothesis, every time. Label them.
- If a test or claim can't distinguish between two explanations, say so.
- Don't tell me something works because it should work — find where someone
  proved it does. See the `feedback_verify_the_test_before_trusting_the_result`
  memory; I got burned by exactly this today.
- Do NOT touch U1 hardware. It's powered off and stays off. This is a
  research + design task only, no deploys, no power-ons.
