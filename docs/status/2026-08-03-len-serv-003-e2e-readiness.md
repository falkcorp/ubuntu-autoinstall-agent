<!-- file: docs/status/2026-08-03-len-serv-003-e2e-readiness.md -->
<!-- version: 1.0.0 -->
<!-- guid: e92d5377-bb20-4276-a50c-347fdbd7c92b -->
<!-- last-edited: 2026-08-03 -->

# len-serv-003 end-to-end readiness assessment

Read-only reconnaissance, 2026-08-03. **Nothing on len-serv-003 was changed.**
Section 3 of the originating brief (provision DASH) was cancelled mid-session;
no BIOS setting, ThinkLMI attribute, DASH state, or reboot was touched. Every
command aimed at the host was a read: `lsusb`, `ls /sys/class/tpm`,
`mokutil --sb-state`, `dpkg-query`, `lsblk`, `systemctl is-active/is-enabled`,
`systemctl cat`, `clevis luks list`, `curl .../adv`.

## TL;DR

The e2e is **not close**, and the reasons are different from the ones in the
standing brief. Three of the six "known state" items were wrong or imprecise
when measured, and the single biggest problem is one nobody listed: **on
`origin/main`, a native-keystore install of len-serv-003 emits a FLAT
`sss` policy — `{"t":2,"pins":{"tang":[×3],"tpm2":{...}}}`** — four shares at
threshold 2, which is exactly the "Tang alone opens it" topology the nested-`sss`
work exists to eliminate. The nested policy lives on **nine local branches, none
pushed, none with a PR**, and **not one of them authors an `unlock_policy` for
len-serv-003**. So the e2e as it stands today would rebuild the box with a
better *layout* and the same *unlock weakness*.

Secondary but blocking: the CockroachDB cluster is running on **one live node**
(U0, nodeID 4) — all three RPi nodes are stopped — so the installer's own
drain/decommission step has nothing healthy to talk to. And there is **no
YubiKey in the machine**, which means the settled policy's two PKCS#11 token
groups cannot be exercised on hardware at all.

Everything about .96's current OS is disposable (the external hardware track
will install Windows over it). What is not disposable, and is recorded here
precisely: the U0 PXE/seed mapping, the cluster state, and our branch state.

## Corrections to the standing brief

These are measured, and they change the plan. Listed before the blocker table
because two of them invalidate premises the table would otherwise inherit.

| Brief said | Measured | Evidence |
|---|---|---|
| len-serv-003 is `172.16.2.96` | **`172.16.3.96/23` on `enp1s0f0`.** `172.16.2.96` does not exist — `ip neigh` shows it `FAILED`, ping unreachable. SSH to `172.16.3.96` returns `len-serv-003`. Same for 001/002 (`3.92`/`3.94`). | `ip -4 -o addr show` on the host; `ip neigh` on U0 |
| Netbooting it as-is rebuilds the wrong layout | **Only if an operator presses `i`.** `mac-6c4b90bcf7f4.ipxe` sets `menu-default boot-local-disk`; `menu.ipxe` uses `choose --timeout 5000`. An *unattended* netboot today falls through to local disk. The trap is real but conditional. | `/var/www/html/ipxe/boot/mac-6c4b90bcf7f4.ipxe`, `/var/www/html/ipxe/menu.ipxe` |
| `clevis-tpm2` would not be installed | **The installer already installs it on `origin/main`.** Not a blocker. | `crates/uaa-core/src/network/ssh_installer/installer.rs:1020` (apt line includes `clevis-tpm2`); `packages.rs:54`; `system_setup.rs:470` |
| CockroachDB node 8 already decommissioned → safe to wipe | Directionally right, precisely unverified — see [Cluster state](#cluster-state). The service is stopped **and disabled**; I could not read `membership=decommissioned`. | `systemctl is-active/is-enabled cockroach` on .96 |
| DASH unprovisioned on .96 | **Confirmed, with a positive control.** | ports below |
| Secure Boot disabled / Setup Mode; no YubiKey | **Both confirmed.** | `mokutil`, `lsusb` |

### DASH — proven, not assumed

```
3.96:623 closed   3.96:664 closed   3.96:16992 closed   3.96:16993 closed
3.92:623 OPEN     3.92:664 OPEN     3.92:16992 closed   3.92:16993 closed
3.94:623 OPEN     3.94:664 OPEN
3.96:22   OPEN    <- control: the host is up and reachable
```

The .92/.94 positive control answers on 623/664, so the .96 negative is a real
absence of a listener, not a routing or firewall artifact. It also re-confirms
the ports are 623/664 and **not** 16992/16993 (closed even on the live boxes).

## Blockers

Split by ownership, as instructed. No remediation is proposed for the external
column — only the state we need handed back.

### Ours — code / config

| # | Blocker | Kind | Evidence | Unblocked by |
|---|---|---|---|---|
| O1 | **`main` emits a FLAT `sss` for native-keystore**: `{"t":2,"pins":{"tang":[3],"tpm2":{"pcr_ids":"7","pcr_bank":"sha256"}}}` — 4 shares, t=2. Two Tang alone satisfy it. This is the topology the nested design forbids. | code | `crates/uaa-core/src/network/ssh_installer/system_setup.rs:1064-1075` builds `tpm2_pin` and splices it into a single flat `pins` object | Merging the nested-`sss` emitter stack (O2) |
| O2 | **The nested-`sss` stack is entirely unpushed and un-PR'd.** 9 local branches; `gh pr list --state open` returns exactly one PR (#168, dependabot). | code | `git rev-parse --verify origin/<branch>` fails for all nine; `gh pr list` | Push + PR + merge, in the order below |
| O3 | **len-serv-003 has no authored `unlock_policy` on ANY branch.** `examples/configs/install/len-serv-003-native-keystore.yaml` is 140 lines on all four policy branches with **0** hits for `unlock_policy`/`unlock_sss`/`pkcs11` (positive control: `tang_threshold` matches 1× on each, so the file is present and grep-able). It authors flat `tang_servers` + `tang_threshold: 2` only. | config | `git show <branch>:examples/configs/install/len-serv-003-native-keystore.yaml` | Authoring the policy for this host; the reference shape is `unlock_policy.tpm2_clevis_peer` in `crates/uaa-core/tests/fixtures/components/unimatrixone.yaml:83` |
| O4 | **The Cockroach cluster is down to one live node.** U0 is nodeID 4 and listening; rpi-serv-001/002/003 all report `cockroach`/`cockroachdb` `inactive` with nothing on 36257/36357/38080 and connection refused on the **host IP** (not just loopback). The installer's drain step (`origin/main` `feat(reinstall): U0 drains cluster membership before wiping a host`) targets a cluster that cannot currently serve. | config/ops | `systemctl is-active` + `ss -ltn` on .45/.46/.47; `ss -ltn` + `journalctl -u cockroachdb` on U0 | Restoring RPi nodes before any drain-dependent step |
| O5 | **The old PXE seed is still the live one.** `/var/www/html/cloud-init/6c4b90bcf7f4/user-data`, md5 `97e49e6d4003df4e31341c68b839d09d`, 226 lines, `storage: layout: {name: lvm}`, packages `cryptsetup`+`lvm2`, and a bind of flat `sss t=2` over Tang ×3. | config | file on U0, read directly | Regenerating the seed from the native-keystore config (see [Netboot path](#netboot-path-today-vs-required)) |
| O6 | **Cluster state is not independently verifiable by us.** `jdfalk` cannot authenticate to CockroachDB: the node cert is rejected as a SQL client cert (`DN: <nil>`, valid for `node`/`localhost`/`rpi-serv-00{1,2,3}`/`unimatrix*` — notably **no len-serv SANs**), the RPi `node.key` is `0600 cockroach`, and `sudo` on U0 needs a password for anything outside the NOPASSWD list. | config | four cert/user combinations tried, all rejected | A readable `client.root` cert or an operator-run `cockroach node status` |

### External — hardware track (decommission → Windows → firmware → DASH)

Not ours to clear. Stated as dependencies only.

| # | Dependency | Kind | Evidence / current reading |
|---|---|---|---|
| X1 | **No remote power.** DASH listener absent on .96 while live on 001/002. Any failed install is a physical trip. | physical | port scan above; `todo.d/2026-07-30-len-serv-003-rebuild-blockers.md` item `dash-003` |
| X2 | **Secure Boot disabled and platform in Setup Mode (no PK).** A PCR7 binding measures an unenrolled state. | physical/firmware | `mokutil --sb-state` → `SecureBoot disabled` / `Platform is in Setup Mode` |
| X3 | **No PKCS#11 token seated.** `lsusb` shows only root hubs, a Genesys Logic hub, and an Intel 9260 Bluetooth adapter. No Yubico VID `1050`. | physical | `lsusb` on .96 |
| X4 | **Firmware level will change.** Currently product `10VHS1X800`, BIOS `M1XKT63A`. A firmware upgrade may reset Secure Boot state, TPM ownership, and DASH config. | physical/firmware | `/sys/class/dmi/id/*` |

### Requirements we have of the hardware track

Short list, in priority order. These are what we need true when the box is
handed back, not requests for how to get there.

1. **Secure Boot enabled and in User Mode (PK enrolled) — not Setup Mode.**
   `main` binds the tpm2 share to `pcr_ids: "7"` in the `sha256` bank. In Setup
   Mode PCR7 attests to nothing meaningful, so the share is theatre. Worse, the
   *ordering* matters: if we bind while Secure Boot is off and the track (or
   anyone) enables it afterwards, PCR7 changes and the tpm2 share is
   permanently dead. **Secure Boot must be settled before we install, not
   after.**
2. **A YubiKey (or the intended PKCS#11 token) physically seated before we
   arrive**, if the token groups are ever to be exercised on this host. Binding
   with a token absent does not fail — it silently collapses shares onto
   whichever token *is* present, `rc=0`, empty stderr. Absent tokens are a
   correctness hazard, not just a coverage gap.
3. **DASH provisioned and answering on 623/664**, matching 001/002. Until then
   every abort step in the runbook below is a physical trip.
4. **TPM 2.0 left enabled and, ideally, cleared once** after the firmware
   upgrade so ownership state is known. Measured today: `tpm0` present at
   `MSFT0101:00`, `tpm_version_major = 2`. *(The "Nuvoton NTC" vendor claim is
   inherited, not verified — `/sys/class/tpm/tpm0/device/description` does not
   exist on this kernel.)*
5. **Confirmation of what Windows did to the ESP and NVRAM boot order**, since
   the box has a single 238.5G NVMe and our installer rebuilds `nvme0n1p1`.

## Netboot path: today vs required

### What .96 gets if it netboots right now

1. dnsmasq proxy-DHCP on U0 (`/etc/dnsmasq.d/ubuntu-netboot.conf`) tags the
   client and hands `http://172.16.2.30/ipxe/boot.ipxe` (for `tag:ipxe`);
   UEFI PXE gets `ipxe.efi`, UEFI HTTP Boot (arch 16) gets
   `http://172.16.2.30/ipxe/grub/grubnetx64.efi`.
   *Note:* the only `dhcp-host` line in that file is
   `6c:4b:90:bc:39:b3,len-serv-001`. There is **no** `dhcp-host` for .96 — its
   identity comes from the per-MAC iPXE file, not DHCP.
2. `/var/www/html/ipxe/boot.ipxe` chains `${boot-dir}mac-${mac:hexraw}.ipxe`
   → `/var/www/html/ipxe/boot/mac-6c4b90bcf7f4.ipxe`.
3. That file sets `hostname len-serv-003` and **`menu-default boot-local-disk`**,
   then chains `menu.ipxe`.
4. `menu.ipxe` runs `choose --timeout 5000 --default boot-local-disk`.
   **Unattended: boots the local disk after 5 s. No install.**
5. **If an operator presses `i`**, `:autoinstall-amd64` boots the 26.04 live
   kernel with
   `ds=nocloud;s=http://172.16.2.30/cloud-init/${mac:hexraw}/` →
   `/var/www/html/cloud-init/6c4b90bcf7f4/` → the **old PlainLuks/LVM seed**
   (O5). That is the trap, and it is one keystroke deep.

Also present in `menu.ipxe`: `item --key w winpe` — the external track's DASH
provisioning entry, explicitly annotated "needs Secure Boot OFF". Worth knowing
it exists so we do not fight the Setup-Mode state that entry depends on.

Seed-directory disambiguation: the iPXE URL is built from `${mac:hexraw}`, so
**only `/var/www/html/cloud-init/6c4b90bcf7f4/` is on the boot path.** The
sibling directories `6c4b90bcf7f4_postinstall/`, `len-serv-003/`, and
`len-serv-003_postinstall/` are **not** reachable from `menu.ipxe` — nothing in
the iPXE chain references a hostname-named path. Treat them as dead weight
unless something outside iPXE reads them.

### What it must be

| Path | Today | Required |
|---|---|---|
| `/var/www/html/ipxe/boot/mac-6c4b90bcf7f4.ipxe` | `set menu-default boot-local-disk` | `set menu-default autoinstall` **only for the install window**, reverted immediately after (leaving it armed means any future reboot reinstalls) |
| `/var/www/html/cloud-init/6c4b90bcf7f4/user-data` | md5 `97e49e6d…`, 226 lines, `layout: {name: lvm}` | regenerated from `examples/configs/install/len-serv-003-native-keystore.yaml` via `uaa config place`, with every `REPLACE_AT_PLACE_TIME` substituted (`validate_config_secrets` fails closed otherwise) |
| unlock policy in the emitted seed | flat `sss t=2` over Tang ×3 | nested `sss` per the settled design (blocked on O1–O3) |

Note the file is `jdfalk`-writable with no sudo, so the flip is trivial — which
is precisely why it is dangerous.

## Cluster state

**Proven:**

- On .96, the unit is `cockroach.service` (**not** `cockroachdb.service` — my
  first check used the wrong name and returned a false `not-found`). It is
  `inactive` and **`disabled`**. The 2026-07-30 inventory records it as
  *enabled*, so it has been deliberately disabled since.
- Nothing listens on 36257/36357/38080 on .96.
- Its unit joins `172.16.2.30:36357,172.16.3.92:36357,172.16.3.94:36357` and
  binds `--listen-addr`/`--sql-addr` to `172.16.3.96` (the documented drift).
- U0 is `nodeID 4`, clusterID `e4d2c601-5df8-4b99-be87-4cac18f9dcfe`, listening
  on all three ports, `--join` limited to `172.16.2.45/.46/.47`.
- rpi-serv-001/002/003 all report `cockroach` **and** `cockroachdb` `inactive`,
  no crdb ports listening, connection refused on the host IP.
  *(I initially tested `127.0.0.1`, which is the documented .003 drift trap;
  re-tested on host IP to avoid the false negative. Both refuse.)*
- All three Tang servers answer `/adv` with **HTTP 200 from .96's network
  position**. This is a network-path measurement and is *not* disposable.

**Hypothesis, not evidence:** that node 8's membership is `decommissioned`. A
stopped, disabled service is consistent with decommissioning and also with a
plain stop. **Someone with cluster credentials must run
`cockroach node status --all` and confirm `membership=decommissioned` +
`is_live=false` before the wipe.** I could not (O6).

**Also unverified — and it is the *cluster*, not .96, that worries me:** with
all three RPi nodes down and only U0 live, a decommission or drain issued now
would run against a cluster that cannot reach quorum.

## Branch and merge state

`origin/main` = `1933d90`.

**`feat/single-disk-native-keystore` does not need merging.** Its tree SHA is
**identical** to `origin/main`'s — the content was already rebased in; the eight
commits are duplicate SHAs of the same subjects. `git merge-base --is-ancestor`
says NOT-MERGED, but `git diff --stat feat/single-disk-native-keystore origin/main`
is empty. Ancestry is stale; content is current. Do not treat it as a blocker.

What actually must land, in dependency order (all currently **unpushed, no PR**):

| Order | Branch | Contributes |
|---|---|---|
| 1 | `feat/unlock-policy-nested-threshold` | the nested/grouped SSS threshold schema; `validate` accepts an authored tree as a real unlock factor |
| 2 | `feat/nested-sss-emitter` | nests the Tang group so the policy is `tang AND tpm2` — the direct fix for O1 |
| 3 | `feat/clevis23-pkcs11-pinning` | opt-in clevis 23 from the 26.10 pocket (the pkcs11 pin needs it); chroot apt path covered |
| 4 | `feat/vm-gate-softhsm-pkcs11` | SoftHSM-backed PKCS#11 VM gate with red-first negative controls |
| 5 | `fix/verify-clevis-share-topology` | `verify` asserts share **topology**, not substrings — this is what would have caught O1 |
| 6 | `feat/nested-unlock-integration` | integration: gates the dracut clevis module on the bind predicate; a tree-only host is bound, not silently skipped |
| 7 | `feat/pkcs11-share-emission` | emits pkcs11 shares at any depth; enforces policy at the **bind**, not only the registry; optional `mechanism` |
| 8 | `fix/clevis-boot-bounded-failclosed` | bounded, fail-closed clevis boot |

`feat/clevis-pkcs11-multitoken-pin` is a merge of 3+4 and carries nothing of its
own. Branches 2/6/7 already contain each other's commits via local merges, so
the graph needs untangling before PRs — do not push them as-is.

**Plus (O3): author an `unlock_policy` for len-serv-003.** None of the eight
does. Without it, merging all eight still leaves .96 on flat `tang_threshold: 2`.

## Runbook

**Entry preconditions — the hardware track has handed the box back.** Do not
start until all of these are true:

- [ ] Windows install and firmware upgrade complete; box handed back explicitly.
- [ ] DASH answering on `172.16.3.96:623` **and** `:664`
      (`nc -z -w3 172.16.3.96 623 664`), with a **working authenticated power
      action proven at least once** — `todo.d` item `dash-auth` records that
      unauthenticated `wsman identify` works on 002 while authenticated
      enumerates come back empty. Unauthenticated identify is **not** sufficient;
      it does not prove we can power the box on.
- [ ] Secure Boot **enabled and in User Mode** (`mokutil --sb-state` reports
      enabled, not Setup Mode) — settled *before* install, per requirement 1.
- [ ] TPM 2.0 enumerated (`/sys/class/tpm/tpm0`, `tpm_version_major` = 2).
- [ ] YubiKey seated, **or** an explicit written decision that token groups are
      out of scope for this host.
- [ ] `cockroach node status --all` shows node .96 `membership=decommissioned`,
      `is_live=false`, run by someone with credentials.
- [ ] RPi Cockroach nodes back up (quorum restored) — otherwise the drain step
      has no healthy cluster.
- [ ] Branches 1–8 merged to `main`; an `unlock_policy` authored for
      len-serv-003; `cargo test` green; the SoftHSM VM gate green on the merged
      tree, **not** on a branch.

Then:

1. **Snapshot the current PXE state.** Copy `mac-6c4b90bcf7f4.ipxe` and
   `cloud-init/6c4b90bcf7f4/user-data` aside with today's date. Record md5s.
   *Abort/recovery: none needed — read-only.*
2. **Place the config.** `uaa config place` from
   `examples/configs/install/len-serv-003-native-keystore.yaml`, substituting
   every `REPLACE_AT_PLACE_TIME` (system NVMe by-id — use the **full by-id
   path**, never `/dev/nvme0n1`, because NativeKeystore appends `-partN`;
   `luks_key`; `root_password`; `install_ca_cert`; livepatch key;
   rollout-agent `database-url`).
   *Abort: `validate_config_secrets` fails closed on a surviving placeholder.
   Fix and re-place. No physical risk.*
3. **Verify the emitted seed before arming anything.** Diff the newly written
   `user-data` against the saved md5 `97e49e6d…`; confirm `storage:` no longer
   says `layout: {name: lvm}`; confirm the clevis bind emits the **nested**
   `sss`, not a flat four-share object. Grep for the literal
   `"pins":{"tang":[` followed by `,"tpm2"` at the same nesting depth — that
   pattern is the O1 defect.
   *Abort: stop here. Nothing has touched the machine. This is the last
   completely free abort point.*
4. **Confirm Tang quorum.** All three of `172.16.2.45/.46/.47` must answer
   `/adv` 200 **from .96's network position**, and `rpi-serv-001` (.45) has a
   history of flapping. Two of three is the bind threshold; require all three
   before proceeding so a single flap does not strand the box.
   *Abort: fix Tang first. Do not proceed on 2/3.*
5. **Drain / confirm decommission.** Re-run `cockroach node status --all`.
   *Abort: if the cluster is unhealthy, stop. Wiping a node out of an
   already-degraded cluster risks the cluster, not just the host.*
6. ⚠️ **Arm the netboot.** Flip `mac-6c4b90bcf7f4.ipxe` to
   `set menu-default autoinstall`.
   **From here on, every failure is potentially a physical trip.**
   *Abort/recovery: flip the file back to `boot-local-disk` immediately. It is
   `jdfalk`-writable with no sudo, so this is fast — but only helps if the box
   has not already started the install.*
7. ⚠️ **Power-cycle via DASH** and watch the install.
   *Abort/recovery: **requires DASH.** If DASH does not respond, this step
   cannot be aborted remotely — it is a physical trip. Do not start this step
   without having proven an authenticated DASH power action in the entry
   preconditions.*
8. ⚠️ **Install runs; the critical moment is the `clevis luks bind`.** The bind
   is non-interactive only because the installer pre-fetches each Tang
   advertisement to `/run/uaa-tang-{i}.adv` and passes it via the `adv` key —
   without it, `clevis luks bind` prompts on `/dev/tty` and fails over SSH,
   leaving the keystore with **no unattended-unlock binding**
   (`system_setup.rs:1049-1062`).
   *Abort/recovery: if the bind fails, the box has an encrypted keystore with
   only the break-glass `luks_key`. It will not auto-unlock. Recovery is
   console access — **physical trip**, or DASH SOL if that works. Have the
   `luks_key` in hand before this step.*
9. ⚠️ **First unattended reboot — the real gate.** The box must reach `sshd`
   without a console.
   *Abort/recovery: if it does not, this is the classic stranded-host case.
   DASH power-cycle + SOL, else physical. **Do not** attempt a second install
   before reading the console — a blind retry destroys the evidence.*
10. **Verify the binding topology on the live host**, not just the config:
    `clevis luks list -d <keystore>` must show the **nested** structure. For
    comparison, .96 today shows the flat form:
    `1: sss '{"t":2,"pins":{"tang":[{"url":"http://172.16.2.45"},{...46},{...47}]}}'`.
    If the new box shows a shape like that, **the e2e failed even though the
    install succeeded.**
11. **Disarm.** Flip `mac-6c4b90bcf7f4.ipxe` back to `boot-local-disk`.
    *Skipping this leaves a machine that reinstalls itself on any future PXE
    boot.*
12. **Verify applications** against
    `docs/len-serv-003-preflight-inventory-2026-07-30.md`, and confirm Cockroach
    came back on the **001/002 flag form** (port-only `--listen-addr`/`--sql-addr`,
    `path=...,attrs=ssd,size=.5`) rather than 003's IP-bound drift.

## What the e2e will NOT cover

Bluntly:

- **The PKCS#11 token groups — groups 2 and 3 of the settled policy — will not
  be tested on hardware.** `lsusb` on .96 shows no Yubico device. They cannot be
  bound without the token physically present, and worse, binding with a token
  absent does **not** fail: it silently collapses the shares onto whatever token
  *is* present, `rc=0`, empty stderr. So this e2e cannot distinguish "token
  groups work" from "token groups silently degraded". Their only coverage
  remains the SoftHSM VM gate (`feat/vm-gate-softhsm-pkcs11`), which is
  simulation, not hardware.
- **PCR7 / tpm2 share semantics**, unless Secure Boot is enabled and in User
  Mode first. Binding in Setup Mode produces a share that measures nothing, and
  which breaks the moment Secure Boot is later enabled.
- **Multi-disk / mirrored native-keystore layouts.** .96 is a single 238.5G
  NVMe with no redundancy by design; the U1 four-drive special-vdev topology is
  untouched by this run.
- **Remote recovery**, until DASH is provisioned *and* authenticated power
  control is proven. An unauthenticated `wsman identify` does not prove we can
  power the box on.
- **Cluster rejoin under realistic conditions**, while the cluster is a single
  live node.
- **Whether a token-absent bind is detected.** Nothing in the runbook can catch
  it on this host, because there is no token to be absent *from*.

## Evidence vs hypothesis

**Evidence (measured this session):** the `172.16.3.96` address; DASH ports with
positive controls; `mokutil` Secure Boot state; `lsusb` token absence; TPM 2.0
presence; `clevis luks list` flat-`sss` slot 1; `clevis-tpm2`/`clevis-decrypt-tpm2`
absent from the *current* install (with `clevis-decrypt-{null,sss,tang}` present
as positive control); `cockroach.service` inactive+disabled on .96; RPi nodes
inactive on host IP; Tang `/adv` 200 ×3; seed md5 `97e49e6d…`; the full iPXE
chain; the flat-`sss` emitter at `system_setup.rs:1064-1075`; branch/tree SHAs.

**Hypothesis (labelled, not proven):**

- That node 8's membership is `decommissioned` (service state is consistent
  with it, but I could not authenticate to confirm).
- That the TPM is a Nuvoton NTC part — inherited from prior notes; the sysfs
  `description` node does not exist here.
- That the RPi Cockroach nodes are down *deliberately* rather than failed. I
  measured that they are down; I did not determine why.

**Explicitly retracted false negative:** I first checked for
`clevis-decrypt-tpm2` inside `/boot/initrd.img-7.0.0-28-generic` via
`lsinitramfs` and got 0 matches — but the **`cryptsetup` positive control also
returned 0, and the total entry count was 0**, because the initrd is `0600
root`-only and `jdfalk` cannot read it. That check proves nothing. The
conclusion still holds by a different, valid route: `clevis-decrypt-tpm2` does
not exist anywhere on the filesystem (`dpkg-query` finds no `clevis-tpm2`
package; `/usr/bin/clevis-decrypt-tpm2` is absent while
`/usr/bin/clevis-decrypt-{null,sss,tang}` are present), and a binary that is not
on disk cannot be in the initramfs. **All of this is disposable anyway** — the
Windows install will replace it.

## Next steps

1. **Fix O1 first** — it is the one defect that would let a "successful" e2e
   ship the wrong security property. Land branches 1, 2 and 5 (5 is the test
   that catches regressions of 1).
2. **Author an `unlock_policy` for len-serv-003** (O3), modelled on
   `crates/uaa-core/tests/fixtures/components/unimatrixone.yaml:83`. Note that
   `lower()` currently drops the nested leaf and the installer derives the D2-B
   tpm2 peer from `storage_mode` — confirm the merged stack changes that, or the
   authored policy will be silently ignored.
3. **Untangle the branch graph** before opening PRs; several branches contain
   each other's commits via local merges.
4. **Get cluster credentials** so `membership` is verifiable without an operator
   in the loop, and **restore RPi quorum**.
5. **Retire the old seed** (`todo.d` item `seed-trap`) rather than relying on
   `menu-default boot-local-disk` as the only guard — it is one keystroke deep
   and the file is writable without sudo.
6. Correct `docs/netboot-autodeploy.md` (`todo.d` item `netboot-doc`); it still
   calls the len-serv-003 `user-data` the known-good template.

## Related

- `docs/len-serv-003-preflight-inventory-2026-07-30.md` — pre-wipe inventory and
  drift audit
- `todo.d/2026-07-30-len-serv-003-rebuild-blockers.md` — `dash-003`,
  `dash-auth`, `seed-trap`, `netboot-doc`, `app-specs`, `crdb-flags`
- `docs/status/2026-08-01-dash-provisioning-complete-context.md` — DASH context
  (external track)
