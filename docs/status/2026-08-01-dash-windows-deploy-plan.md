<!-- file: docs/status/2026-08-01-dash-windows-deploy-plan.md -->
<!-- version: 2.0.0 -->
<!-- guid: 3f9c2a71-84be-4d52-9c07-2ab6f15d83c4 -->
<!-- last-edited: 2026-08-04 -->

# DASH on len-serv-003 — Windows Deploy Plan

Supersedes the "next steps" of `2026-08-01-dash-provisioning-complete-context.md`.
Everything below was measured this session unless marked INFERENCE.

---

## Findings this session (all measured)

### 1. strace resolved the blocker's nature
`clienttool` is **not** hung on a missing file or permission. It sits in a clean
3-second poll loop:

```
ioctl(3, _IOC(_IOC_NONE, 0x89, 0xf1, 0), ...) = 0     <- SIOCRTLTOOL, SUCCEEDS
clock_nanosleep(CLOCK_REALTIME, 0, {tv_sec=3}, ...)   <- repeat forever
```

The driver leg works every time. The **firmware never answers the OOB mailbox**.
Message order in the binary is `FWVersion` -> `Dash is/is not running`; neither
"running" string is ever printed, so it stalls inside `getDashStatus`.

`clienttool` takes only ONE argument. The `(0)` in its banner is the interface
index, not a debug level — there is no verbose mode to enable.

### 2. The OOB engine is alive — hypothesis (b) is dead
`/proc/net/r8168/enp1s0f0/debug/driver_var`:

```
chipset        27          RTL8168EP/8111EP
HwSuppDashVer  0x2         <- DASH TYPE_2
DASH           0x1         <- ENGINE ACTIVE
```

`_rtl8168_check_dash()` passed both gates (OCP `0x128` BIT_0 set, FW version
neither 0 nor 0xffffffff). So DASH is genuinely enabled in firmware.

**Refined hypothesis (INFERENCE, still unproven):** ours is DASH **TYPE_2**
silicon; the P8 toolkit targets firmware 5.1.23 which is **TYPE_3** (the
CMAC-sibling-function path visible at `r8168_n.c:26280`). The 2023 `clienttool`
is likely polling a TYPE_3 mailbox that TYPE_2 hardware does not implement.

`rtltool.c` confirms the driver is a **dumb pipe** — `RTL_READ_OOB_MAC` is a raw
`rtl8168_ocp_read()`. The entire mailbox protocol lives in userspace. So the fix
must be a different userspace tool, not a driver change.

### 3. The Windows payload is real and is the right vintage
All three copies on windows-gpu report **FileVersion 1.0.6.0**, 173,872 bytes:

```
C:\dashwork\harvest\DASHConfigRT.exe
C:\dashwork\x64\extracted\DASHConfigRT.exe
C:\Program Files (x86)\AMD\Provisioning Console\Tools\DASHConfig-Realtek\DASHConfigRT.exe
```

`Provisioning_Console_setup-5.1.0.541` is the **installer's** version; the
Realtek tool it bundles is still v1.0.6.0 — contemporary with our 1.3.11 /
TYPE_2 firmware. **This is the whole basis for expecting a different outcome
than the Linux tool.**

### 4. Both feared one-way doors are safe
- `mokutil --sb-state` -> **SecureBoot disabled**, *Platform is in Setup Mode*.
  The PCA2023 / UEFI-CA-2011 signing concern does not apply. `wimboot` being
  unsigned is also fine.
- `menu-default boot-local-disk` intact, 5s timeout; `:winpe` needs an explicit
  `w` keypress. An unattended reboot falls through to local disk.

### 5. The one-shot lever already exists (operator-identified)
`boot.ipxe` chains per-machine overrides by descending specificity:
`hostname-*` -> `uuid-*` -> `mac-*` -> `pci-*` -> `chip-*` -> menu.
len-serv-003's file is `/var/www/html/ipxe/boot/mac-6c4b90bcf7f4.ipxe`:

```ipxe
set menu-default boot-local-disk
set hostname len-serv-003
chain --replace --autofree ${menu-url}
```

Flip that one line to `winpe`, reboot, then **revert it on U0**. The revert is
server-side, so a WinPE that fails to network and dead-man-reboots lands back on
`boot-local-disk`. No reboot loop is possible.

### 6. CockroachDB — already handled, no risk
len-serv-003 was **node 8** of cluster `e4d2c601-5df8-4b99-be87-4cac18f9dcfe`,
and node 8 is **already `decommissioned`** with zero replicas (since ~Jul 28).
Its daemon had been retrying a join it can never be allowed to complete — that,
not a network fault, is what the "unable to contact the other nodes" warnings
were. **Nothing to decommission.**

Action taken: `systemctl disable --now cockroach.service` on .96.
Cluster verified healthy after: nodes 4 (U0), 5 (.94), 6 (.92), 9 (U1) all
`is_available=true is_live=true`.

Admin access note: no `client.root.crt` existed anywhere. Minted one on U0 at
`/home/jdfalk/crdb-admin-certs/` (mode 700) from the cluster CA at
`/home/jdfalk/.cockroach-ca/ca.key`, after verifying the CA SHA-256 fingerprint
matches the nodes' `ca.crt` exactly.

### 7. The VM gate did NOT prove the Realtek driver
`virsh dumpxml winpe-dash-test` shows `<model type='e1000e'/>` with the MAC
**spoofed** to `6c:4b:90:bc:f7:f4`. So the MAC gate fired and DHCP worked via
WinPE's in-box **Intel** driver. The injected Realtek `rt640x64` driver has
**never actually loaded**. This is the single largest remaining risk.

### 8. The PXE serving bug is live and reproduced
```
http://172.16.2.30/winpe/boot.wim   -> code=200 len=11256   <- SPA index.html
```
Real file on disk is 506,400,047 bytes. `/var/www/html/winpe/` still has no
nginx location block. `${base-url}` is also unset at the `:winpe` label.
`/home/jdfalk/fix-winpe-pxe-paths.sh` addresses both and self-verifies with a
size assertion — reviewed, looks correct, **not yet run**.

---

## Plan

### Phase 0 — DONE
- [x] Confirm cockroach state; disable service on .96; verify cluster health.

### Phase 1 — DONE 2026-08-04. All gates cleared, nothing rebooted.

**1. Realtek driver hardware-ID match — CLEARED.** `rt640x64.inf` holds 5,863
`DEV_8168` entries across 35 revisions. Our exact ID is present:

```
%RTL8168.DeviceDesc% = RTL8168EP.ndi, PCI\VEN_10EC&DEV_8168&SUBSYS_313017AA&REV_0E ;Lenovo
```

It resolves to the **RTL8168EP** install section, corroborating the Linux
`driver_var` reading of `chipset 27 = RTL8168EP/8111EP`. The earlier
"everything is REV_01" alarm was a `head`-sampling artifact.
Both drivers were already in the WIM: `oem0.inf`=rt640x64 (Net),
`oem1.inf`=rtvdevx64 (Multifunction).

**2. The old `provision.ps1` was a corrupt placeholder.** Attributes were
`SparseFile, ReparsePoint`; the directory entry claimed 5,544 bytes but every
read and copy failed with *"The file cannot be accessed by the system."* It
could never have executed. Deleted and replaced.

**3. WinPE restructured as a thin bootstrap.** Phase logic now lives on U0, so
switching phases never requires rebuilding a 500 MB image:

| File | Location | Role |
|---|---|---|
| `startnet.cmd` | WIM `System32` | starts watchdog FIRST, wpeinit, runs bootstrap, **always reboots** — no `cmd /k` |
| `watchdog.ps1` | WIM `X:\dash` | dead-man switch; deadline file + 4 h hard cap; **fails closed** (unreadable deadline = reboot) |
| `bootstrap.ps1` | WIM `X:\dash` | MAC gate, DHCP wait, fetches+validates payload |
| `payload.ps1` | U0 `/isos/winpe-payload/` | swappable phase script; currently **Phase 2 recon, writes nothing** |

**4. PXE serving fixed and verified by SIZE, not status code.**
`fix-winpe-pxe-paths.sh` ran; it exits 1 due to a bug in its own verify
function, but every URL was confirmed manually:

```
ipxe/wimboot          200   76,064
isos/winpe/bootmgr    200  478,366
isos/winpe/BCD        200  262,144
isos/winpe/boot.sdi   200 3,170,304
isos/winpe/boot.wim   200 512,725,881
```

New `boot.wim` SHA-256 `D0EEBB32…38DC3`, verified **identical on windows-gpu,
the transit host, and U0**. Old image kept as `boot.wim.old-506400047`.

Note: nginx serves only `/cloud-init`, `/ubuntu`, `/isos`, `/uaa`, `/ipxe`.
A new top-level dir returns the SPA with **HTTP 200** — re-confirmed
deliberately this session (`/winpe-payload/probe.txt` → 200, 11,256 bytes for a
310-byte file). The payload therefore lives under `/isos/`. U0's sudo needs a
password, so no nginx location block was added.

**5. One-shot arming script staged** at `/home/jdfalk/arm-winpe-oneshot.sh` on
U0 (not run). It checks Tang quorum, flips `mac-6c4b90bcf7f4.ipxe` to
`menu-default winpe`, waits for iPXE to fetch it, then **auto-reverts via an
EXIT/INT/TERM trap** — so a WinPE reboot loop is structurally impossible.

**6. The Aug 2 reboots were real power-offs.** `journalctl --list-boots` shows
three cycles, with gaps of 2h11m, 194s, and 86s; boot -1 ended in
`systemd-poweroff.service` → *System Power Off*. Someone was physically at the
machine. DASH ports stayed closed throughout. Caveat: a soft poweroff keeps
+5VSB on the NIC, so this is not strictly an AC-cycle test — **open question
for the operator: was the cord actually pulled during the 2h11m gap?**

### Phase 1 (original checklist, superseded by the above)
1. **Realtek WinPE driver hardware-ID match.** BLOCKING.
   `rt640x64.inf` (DriverVer 05/10/2019) entries observed so far are all
   `&REV_01`; our NIC is `10ec:8168` **rev 0e**, subsystem `17aa:3130`.
   Enumerate every `DEV_8168` line and confirm one matches rev `0E` or is
   revision-agnostic. If nothing matches, WinPE gets no network on real
   hardware and the PXE approach is off the table until a newer INF is sourced.
2. Run `/home/jdfalk/fix-winpe-pxe-paths.sh` on U0. Require its `ALL URLS OK`
   and confirm `boot.wim` returns ~506 MB, not 11 KB.
3. Add a **dead-man switch as the first lines of `startnet.cmd`**: bounded
   ping/HTTP loop to 172.16.2.30, `wpeutil reboot` on failure. Rebuild the WIM.
4. Re-verify Tang quorum immediately before the reboot (>=2 of 3 on `/adv`).
   .45 flaps — do not trust a single earlier check.

### Phase 1b — VM GATE, DONE 2026-08-04. Boot chain proven end to end.

The pre-existing VM gate booted WinPE from a **CDROM ISO**, so the
`wimboot` + BCD + `boot.sdi` + `boot.wim` chain that PXE actually uses had
**never been executed once**. Closed that gap by building
`/var/www/html/isos/ipxe-winpe-test.iso` — a UEFI iPXE ISO whose
`autoexec.ipxe` chains the *real production URLs* on U0. VM is OVMF with
secure-boot off, matching .96.

**First run FAILED — and this is exactly why the gate was worth running.**

```
Get-NetAdapter : The term 'Get-NetAdapter' is not recognized ...
At X:\dash\bootstrap.ps1:40 char:10
[*] bootstrap returned - rebooting in 10s.
```

**WinPE does not ship the NetAdapter / NetTCPIP PowerShell modules.** They come
with full Windows; no WinPE optional component provides them. PowerShell itself
works fine, and `wpeinit` had already obtained a DHCP lease — the network was
up, the script just used the one API family WinPE cannot offer.

Fix: new `X:\dash\netinfo.ps1` exposing `Get-PeNicMac` / `Get-PeIPv4` /
`Get-PeNicDescription` via `Win32_NetworkAdapter` and
`Win32_NetworkAdapterConfiguration`, with a `getmac`/`ipconfig` text fallback.
All three scripts now use it.

**The safety architecture held under this real failure**: bootstrap died →
returned → `startnet.cmd` rebooted on schedule. On .96 the reverted mac file
would have sent it straight back to Ubuntu. No hang, no trip.

**Second run — full pass.** Measured from U0's nginx access log:

```
GET /ipxe/wimboot            200        76,064
GET /isos/winpe/bootmgr      200       478,366
GET /isos/winpe/BCD          200       262,144
GET /isos/winpe/boot.sdi     200     3,170,304
GET /isos/winpe/boot.wim     200   516,713,951
GET /isos/winpe-payload/checkin?event=nic-ok&mac=6C:4B:90:BC:F7:F4&ip=172.16.6.53
GET /isos/winpe-payload/payload.ps1   200   4,526
GET /isos/winpe-payload/checkin?event=recon&...&ip=172.16.6.53&disks=0&parts=0
wimboot fetches: 2   nic-ok: 1   recon: 1   watchdog reboots: 0
```

Proven: PXE chain loads; `startnet.cmd` runs; MAC gate passes on the target MAC;
WinPE-safe network detection works; payload fetch passes the size/HTML
assertions; recon executes and reports; **the machine reboots on its own**
(2 wimboot fetches). `watchdog reboots: 0` is correct — the normal path finished
before the backstop was needed.

Empty `drv=`/`disks=0` are correct for the VM: e1000e NIC, no disk attached.
`wow64=True` confirms the old WOW64-graft DLLs are still in the image; harmless,
and the graft is a known dead end regardless.

**NOT proven, and unprovable in a VM:** that `rt640x64` binds to the real
RTL8168EP. QEMU cannot emulate that silicon. That is the single remaining
unknown, and it is the entire purpose of the Phase 2 boot on .96.

Current boot.wim: **516,713,951 bytes**, SHA-256 `134CE74E…AE43`, verified
identical on windows-gpu and U0. mac file confirmed back at `boot-local-disk`.

### Phase 2 — Non-destructive WinPE boot test
5. Flip `mac-6c4b90bcf7f4.ipxe` to `set menu-default winpe`.
6. Reboot .96. **Revert the mac file on U0 as soon as iPXE has fetched it**
   (watch the nginx access log), so any later reboot falls back to Ubuntu.
7. Success = WinPE checks in over the network from the real Realtek NIC.
   Failure = dead-man reboots to Ubuntu, nothing lost. **This phase writes
   nothing to disk and is fully reversible.**

### Phase 3 — Windows install (FIRST DESTRUCTIVE STEP)
8. Preserve off .96 first: `dashkit/`, `config1.xml`, and confirm
   `/home/jdfalk/dash-credentials-len-serv-003.txt` on U0 is intact (194 bytes,
   0600 — verified this session).
9. Apply Windows from `/mnt/bigdata/apps/`. Recommended:
   `26100.1742...CLIENT_LTSC_EVAL_x64FRE` — leanest, has WOW64, best odds of
   accepting a consumer Realtek NIC INF. Server 2025 (`26100.32230`) is the
   riskiest for driver install.
10. Inject `rt640x64` into the applied image so Windows boots with network.

### Phase 4 — Provision DASH
11. Run `DASHConfigRT.exe` **v1.0.6.0** with the `<MANAGEMENTTARGET>`-wrapped
    `config1.xml` and the credentials from U0.

### Phase 5 — Firmware
12. Check whether the P8 **Windows** Realtek package ships an in-band NIC
    firmware updater. If it does, consider running it **before** step 11 — if
    the blocker really is a version mismatch, that is the actual fix.
13. BIOS is already newest (M1XKT63A). **Do NOT run the Nuvoton TPM update**
    without re-checking `clevis luks list` — it is key-destructive.

### Phase 6 — Verify
14. `nmap -Pn -p 623,664 172.16.3.96` from U0 -> both open.
15. Authenticated WS-MAN verb (NOT `wsman identify`, which is unauthenticated
    and succeeds with any password).

### Phase 7 — Wipe and rebuild
16. NativeKeystore ZFS layout. DASH creds survive in NIC firmware.

---

## Standing hazards (unchanged)
- No remote power. A failed boot = physical trip. This is the entire point.
- Tang quorum >=2 of 3 before every reboot; .45 flaps.
- `pkill -x clienttool`, never `-f`.
- Run `clienttool` under a pty or it prints nothing.
- Do not retry the WinPE WOW64 graft — verified dead.
- If a Lenovo/AMD page 403s, ask the operator; do not conclude absence.

## Open question for the operator
A NIC DASH engine often needs a **cold AC power cycle**, not a warm reboot, to
re-initialize. If anyone is at the rack, a full power-off/on of .96 is worth
doing before the Windows detour — it is free and could change the outcome.
