<!-- file: docs/status/2026-08-01-dash-provisioning-complete-context.md -->
<!-- version: 1.1.0 -->
<!-- guid: 6b1d90fa-2c47-4e83-a5d6-71f0e3c48ba9 -->
<!-- last-edited: 2026-08-01 -->

# DASH Provisioning — Complete Context Handoff

**Status as of 2026-08-01 21:20 EDT.** This document is a full brain-dump so a
fresh agent can continue with zero prior conversation. Everything here was
measured unless explicitly marked as inference.

---

## 1. The Goal

Enable **DASH** (DMTF Desktop and mobile Architecture for System Hardware)
out-of-band management on **len-serv-003** so it can be power-cycled and
rebuilt remotely, instead of requiring a physical trip.

DASH gives remote power on/off/reset, boot-to-BIOS, serial-over-LAN and KVM —
independent of the OS, working even when the machine is powered off, because it
lives in the **NIC firmware**.

**Why it matters:** len-serv-003 is scheduled for a wipe/rebuild to the
single-disk NativeKeystore ZFS layout. Without remote power, every failed
install attempt is a physical trip to the rack.

**Critical property:** DASH credentials live in **NIC firmware, not on disk**,
so they **survive an OS wipe**. This is proven — len-serv-001/002 answer on
DASH ports while having no DASH tooling anywhere on their filesystems.
Therefore: provision DASH FIRST, then wipe.

---

## 2. Hardware and Network Inventory

| Host | IP | Hardware | Notes |
|---|---|---|---|
| **len-serv-001** | 172.16.3.92 | ThinkCentre M715q 2nd Gen | DASH **already provisioned**, creds UNKNOWN |
| **len-serv-002** | 172.16.3.94 | ThinkCentre M715q 2nd Gen | DASH **already provisioned**, creds UNKNOWN |
| **len-serv-003** | 172.16.3.96 | ThinkCentre M715q 2nd Gen | **THE TARGET** — DASH unprovisioned |
| U0 / unimatrixzero | 172.16.2.30 | Supermicro X9DR3/i-F | "the server". PXE/netboot host, libvirt VM lab, all downloads staged here |
| U1 / unimatrixone | 172.16.2.35 | Supermicro X10DSC+ | In production, IPMI 172.16.2.218 ADMIN/ADMINADMIN |
| windows-gpu | 172.16.3.22 | Windows 11 Pro build 26200 | SSH via `~/.ssh/id_ed25519_windows`, user `jfg\jdfalk`. **SSH sessions land ELEVATED** |
| rpi-serv-001 | 172.16.2.45 | Raspberry Pi | Tang server. **FLAPPING — intermittently drops off network** |
| rpi-serv-002 | 172.16.2.46 | Raspberry Pi | Tang server |
| rpi-serv-003 | 172.16.2.47 | Raspberry Pi | Tang server |

- RPis use username **`ubuntu`**, everything else uses **`jdfalk`**.
- **172.16.3.94 and 172.16.3.95 are THE SAME BOX** (len-serv-002). A sweep
  using both silently double-covers it and misses len-serv-001 (172.16.3.92).
- Fleet is standardising on **Ubuntu 26.04 LTS**. This is normal, not an
  anomaly. Do not treat it as a risk factor.

### len-serv-003 specifics (measured)
- **Ubuntu 26.04 LTS, kernel 7.0.0-28-generic, gcc 15.2.0**
- MT-M `10VH-S1X800`, S/N `MJ0AAHEE`, machine type **10VH**
- NIC: `enp1s0f0`, MAC `6c:4b:90:bc:f7:f4`, PCI **`[10ec:8168]` rev 0e**,
  Realtek **RTL8111EPP**
- BIOS **M1XKT63A dated 04/11/2024 — already the newest available**
- BIOS `DASH Support = Enabled` (confirmed via ThinkLMI, possible values
  `Disabled;Enabled`)
- Disk: single `nvme0n1` 238.5G — p1 1G vfat `/boot/efi`, p2 2G ext4 `/boot`,
  p3 235.4G **crypto_LUKS → LVM2 → ubuntu--vg-ubuntu--lv ext4 `/`**
  (i.e. ext4-on-LVM-**on-LUKS**, not plain ext4-on-LVM)
- **NO Windows installed.** `Boot0000* Windows Boot Manager` in NVRAM is a
  STALE entry — the ESP contains only `EFI/BOOT` and `EFI/ubuntu`, no NTFS
  anywhere.
- BootOrder: `0007(PXE), 0001(Ubuntu), 0005(USB), 0006(CD), 0000(stale Windows)`

---

## 3. What DASH Is and How It Works

- **Ports 623 (HTTP) and 664 (HTTPS)** — measured and confirmed.
  **NOT 16992/16993** — that is Intel AMT and does not apply to this hardware.
  Older docs in this repo claiming 16992 are WRONG (see §11).
- Protocol is **WS-MAN** over those ports.
- `wsman identify` succeeds with ANY password — it is **unauthenticated**.
  Do NOT use it as a credential test. Only an authenticated verb distinguishes
  "listening" from "usable".
- Provisioning is **in-band** (from the host OS, through the NIC driver).
  Management afterwards is **out-of-band** (over the network, OS-independent).
- **The chicken-and-egg:** an unprovisioned NIC has nothing listening on
  623/664, so there is no network path in. The first write MUST come from
  inside the host via the driver.

### Current DASH port state (measured)
| Host | :22 | :623 | :664 |
|---|---|---|---|
| len-serv-001 .92 | open | **OPEN** | **OPEN** |
| len-serv-002 .94 | open | **OPEN** | **OPEN** |
| len-serv-003 .96 | open | closed | closed |

---

## 4. THE CRITICAL DRIVER FACT

`DASHConfigRT` / `clienttool` reach the DASH firmware through a **private
ioctl**: `SIOCRTLTOOL` = `SIOCDEVPRIVATE+1`, with an `RTLTOOL_*` command set
(`READ_MAC`, `WRITE_MAC`, `READ_PHY`, `READ_EPHY`, `READ_ERI`, `READ_PCI`,
`READ_EEPROM`, …) defined in `DashDriver/src/rtltool.h`.

- **`r8169`** = in-tree mainline driver. Covers the whole RealTek PCIe GbE
  family. Deliberately exposes only standard netdev/ethtool interfaces.
  **Does NOT implement RTLTOOL.**
- **`r8168`** = Realtek's out-of-tree vendor driver for the same silicon. Ships
  `rtltool.c/h`, `r8168_dash.h`, `r8168_asf.c/h`, `r8168_realwow.h`,
  `rtl_eeprom.c`. **Implements RTLTOOL. Required for provisioning.**

Naming is historical: RTL8169 was the original PCI gigabit chip and the
in-tree driver kept its name while being extended to cover the newer PCIe
8168/8111/8125 parts. Our PCI ID is `10ec:8168` but mainline calls its driver
`r8169`.

**Proof by A/B (same command, same pty, only driver changed):**
```
r8169 → [main] FWVersion =                      ← EMPTY
r8168 → [main] FWVersion = 1.3.11. a552218      ← POPULATED
```

**Important corollary:** `r8168` is only needed for the *provisioning write*.
Once DASH is configured it is managed out-of-band and the driver is irrelevant.
That is why .92/.94 answer on 623/664 today while running plain `r8169`. After
provisioning, .96 can be reverted to `r8169` with no loss.

---

## 5. The Tooling — Where It Came From, What Works

### The jackpot source
```
https://download.lenovo.com/pccbbs/thinkcentre_drivers/lenovo_thinkstation_p8_device_drivers.iso
```
9,572,421,632 bytes. **Open, no bot protection.** Downloaded to U0 at
`~/ai/p8iso/p8.iso`.

Inside: `LINUX/ETHERNET/Realtek/L7ETN002DA.zip` (1.69 MB, sha256
`364bc5a49c9facf2ae53ea81ee21dde9b9a3eff856b773e236a9fd9bbeeff04a`) containing:

| Path | What |
|---|---|
| `DASHConfigRT/DASHConfigRT` | **ELF 64-bit x86-64, statically linked**, v1.05, "First official released Linux version" (2023-08-30) |
| `DASHConfigRT/config1.xml` | vendor sample config |
| `DASHConfigRT/DASHConfig_RT_guide.pdf` | tool guide |
| `DashDriver/src/*` | **r8168 driver source with DASH support** |
| `DashDriver/autorun.sh` | build+install script (SEE HAZARD §9) |
| `ClientTool/clienttool` | ELF 64-bit x86-64 static — the DASH client daemon |
| `ClientTool/dash_client` | bash wrapper; auto-discovers the `r8168` interface |
| `ClientTool/dash_client.service` | systemd unit, `Restart=always` |
| `ClientTool/install.sh` | installs/removes the service |

**The P8 uses the SAME NIC as us — `Realtek RTL8111EPP`** — which is why this
package applies. Officially supported OSes include Ubuntu 22.04 LTS.

### The authoritative guide
```
https://download.lenovo.com/pccbbs/thinkcentre_pdf/ts_p8_dash_configuration_guide_v.10.pdf
```
1,746,148 bytes, 24 pages, v1.0 2024-12-02. On U0 at
`~/ai/p8iso/ts_p8_dash_configuration_guide_v.10.pdf`.
Sections: 2 = BIOS enablement, 4 = Linux driver install, 6 = Linux DASH config,
7 = start client service, 8 = AMD Management Console, 10 = removal.

### THERE IS NO x64 WINDOWS DASHConfigRT
Verified by reading **PE machine type**, not filenames:

| Binary | Arch |
|---|---|
| `DASHConfig-Realtek\DASHConfigRT.exe` | **x86** |
| `DASHConfig-Broadcom\DASHConfig.exe` | **x86** |
| `DASHConfig-Marvel\AqDashConfig-0.32.0.exe` | **x86** |
| `AIM-TProvisioningApp.exe` | x64 |
| `dashcli.exe` (AMD Manageability API) | x64 |

Lenovo pages titled "…for Windows 11 **(64-bit)**" describe the **target OS**,
not the binary. A user-supplied package (`u4etn00fw17ap.exe`) was hash-compared
against the AMC copy: **byte-identical**, sha256
`3EAB1223F2F4030E54CFD4C54A021C47CF2E7EB21CE838921335933340249EB2`,
173,872 bytes, x86. **Stop chasing an x64 build; it does not exist.**

### Where the Windows tool comes from
`Provisioning_Console_setup-5.1.0.541-AMD.exe` from `download.amd.com/software/`
(open, unauthenticated). Install silently with `/s /v"/qn /norestart"`. Lands at
`C:\Program Files (x86)\AMD\Provisioning Console\Tools\DASHConfig-Realtek\`.
**NOT in AMD Management Console 14.0** — the Lenovo CDRT guide's AMC path is
from an older release.

InstallShield gotcha: `/s /extract_all:` and `/stage_only` are InstallShield
*Suite* flags — on these MSI-wrapper installers they **hang forever** waiting on
a UI. Only `/s /v"/qn"` works. If one hangs: `Get-Process AMC | Stop-Process`.

---

## 6. The XML Schema — GET THIS RIGHT

The config REQUIRES a **`<MANAGEMENTTARGET>` wrapper**. An earlier attempt
omitted it (nesting `GLOBAL`/`USERS` directly under the root) which would have
failed to provision even if the tool had run. Verified against the vendor's own
`config.xml` and `ConfigExample.xml`.

```xml
<?xml version="1.0" encoding="utf-8" ?>
<DASHPROVISIONSETTINGS>
    <MANAGEMENTTARGET>
        <GLOBAL>
            <HTTPS><ENABLESUPPORT>true</ENABLESUPPORT><TCPIPPORT>664</TCPIPPORT></HTTPS>
            <HTTP><ENABLESUPPORT>true</ENABLESUPPORT><TCPIPPORT>623</TCPIPPORT></HTTP>
        </GLOBAL>
        <USERS>
            <USER>
                <USERID>dashadmin</USERID>
                <PASSWORD>SEE CREDENTIALS FILE</PASSWORD>
                <ENABLE>true</ENABLE>
                <ROLES><ROLE>Administrator Role</ROLE></ROLES>
            </USER>
        </USERS>
    </MANAGEMENTTARGET>
</DASHPROVISIONSETTINGS>
```

- `<ROLE>` must be exactly one of: `Administrator Role`, `Operator Role`,
  `Read Only Role`. Max 10 users, max 3 roles per user.
- `ConfigExample.xml` also documents an `<ACTIVEDIRECTORY>` block.
- Vendor default creds are `Administrator` / `Password` (also seen:
  `Administrator` / `Realtek`). Both were tried against .92/.94 → **401**.
  Those two have genuinely non-default credentials.

### Credentials for len-serv-003
Generated 2026-08-01. Username **`dashadmin`**.
**Password is stored 0600 at `/home/jdfalk/dash-credentials-len-serv-003.txt`
on U0 (172.16.2.30).** Deliberately not inlined here because this file lives in
a git repo. The live config with the real password is at
`/home/jdfalk/dashkit/DASHConfigRT/config1.xml` on len-serv-003 (mode 600).

---

## 7. CURRENT STATE OF len-serv-003 (as of 21:20 EDT 2026-08-01)

### What is DONE and WORKING
- `build-essential`, `net-tools` installed. `linux-headers-$(uname -r)` were
  already present; `/lib/modules/$(uname -r)/build` exists.
- **`r8168` driver BUILT and INSTALLED and PERSISTENT.** Survived a reboot —
  loads from initramfs. `r8168.ko` v**8.055.00**, vermagic `7.0.0-28-generic`,
  alias `pci:v000010ECd00008168`.
  `autorun.sh` renamed `r8169.ko.zst` → `r8169.zst.bak`.
- **DASH firmware is REACHABLE**: `FWVersion = 1.3.11. a552218`
- `dash_client.service` installed, enabled, active (auto-starts at boot).
- Toolkit at **`/home/jdfalk/dashkit/`** on len-serv-003
  (`DASHConfigRT/`, `ClientTool/`, `DashDriver/`).
- `config1.xml` written with correct schema, mode 600.
- Initramfs verified to contain `r8168.ko`, `clevis`, `clevis-pin-tang`,
  `clevis-pin-sss`, `systemd-networkd`, `crypt`, `dm`, `lvm`,
  `systemd-cryptsetup`. Kernel cmdline has `rd.neednet=1 ip=dhcp` (DHCP, NOT a
  hardcoded interface name — so the interface-rename hazard does not apply).

### THE BLOCKER — unresolved
`clienttool` reads the firmware version then **stalls permanently**. It never
reaches `[getIpInfoFromOS]` / `[setFWIpInfo]`, and never creates the FIFO
**`/tmp/DasH`** that `DASHConfigRT` writes to.

Observed every time, including after a clean reboot, with no competing
processes, over 60-second runs:
```
[main] Client tool start with NIC inferface enp1s0f0 (0) ...
[enableOOBReq] (RTLTOOL_READ_MAC), g_ifname = enp1s0f0
[readFwVer] Requiring FW Version...
[main] FWVersion = 1.3.11. a552218
                                        ← STOPS HERE FOREVER
```
Process state: `S (sleeping)` in `hrtimer_nanosleep`, only fds 0/1/2 + one
socket. It is idling, not crashed.

Consequently `DASHConfigRT` fails:
```
Start ...
[send_req]Open write pipe failed 2          ← ENOENT, /tmp/DasH absent
DASHConfigRT Version (v1.05)
Dash Firmware Version ()                    ← empty; comes via the pipe
RtkDashClient Version ()
sh: 1: net: not found                       ← Windows leftover, harmless
Error: Fail to config!
```

**NOTHING HAS BEEN WRITTEN TO THE NIC FIRMWARE.** Both attempts failed before
touching it. This is safe to retry.

### Leading hypothesis (NOT proven)
**Firmware generation mismatch.** The P8 toolkit (Oct 2023) targets DASH
firmware **`5.1.23.fb776e53`**; our 2018-era NIC runs **`1.3.11.a552218`**.
The basic handshake works because it is a plain driver ioctl, but the
higher-level OOB mailbox negotiation may use a protocol the old firmware does
not speak. The odd formatting of our version string (`1.3.11. a552218`, with a
stray space) vs the guide's clean `5.1.23.fb776e53` weakly supports this.

### Ruled out (all measured, do not re-test)
- BIOS `DASH Support` — **Enabled**
- BIOS version — **already newest** (M1XKT63A 04/11/2024)
- Orphaned/competing `clienttool` processes — **none**
- `PrivateTmp` on the service hiding the FIFO — **PrivateTmp=no**
- Missing `net-tools`/`ifconfig` — **present**
- IPv6 disabled — **enabled**, both global and link-local present
- A reboot fixing it — **it did not**
- No DASH firmware update exists for MT 10VH (18 catalog packages, only a
  driver). The AMC "Firmware Upgrade" button is out-of-band → chicken-and-egg.

### Next diagnostic not yet done
`strace` the stalled `clienttool` to identify the exact syscall it idles on
after `readFwVer`. This distinguishes "firmware never answers the OOB mailbox"
from a mundane missing-file/permissions cause.

---

## 8. THE PLAN (operator-directed, 2026-08-01)

1. ~~Reboot and retry~~ — **DONE, did not help.**
2. **Deploy Windows to len-serv-003**, scripting the install with **WinPE**.
   Operator is downloading three ISOs and will place them on **bigdata**.
3. Under real Windows, run the **M715q-era `DASHConfigRT.exe` v1.0.6.0** — the
   x86 binary that is contemporary with our `1.3.11` firmware, and which cannot
   run under WinPE (no WOW64) but runs fine on full Windows.
4. Update everything on the machine (firmware etc.) while Windows is on it.
5. Then wipe and rebuild to the NativeKeystore ZFS layout. **DASH credentials
   survive the wipe** because they live in NIC firmware.

**WinPE IS NOT DEAD.** What is dead is running *that x86 binary* under WinPE
amd64. WinPE remains the scripted-install delivery mechanism and is already
built and boot-proven (see §10).

---

## 9. HAZARDS — READ BEFORE ACTING

### `autorun.sh` unloads the NIC BEFORE it compiles
```sh
rmmod r8169                       # ← NIC driver GONE here
make all 1>>log.txt || exit 1     # ← build happens AFTER
```
A build failure leaves **no NIC driver loaded**. Over SSH that is instant loss
of connectivity and a physical trip. **Build first** (`make all` while the NIC
is up), then arm a dead-man switch, then swap:
```
sudo systemd-run --on-active=5min --unit=nic-rescue /bin/sh -c \
  "modprobe r8169; ip link set enp1s0f0 up; systemctl restart systemd-networkd"
```
Cancel with `systemctl stop nic-rescue.timer` once SSH is confirmed back.
Recovery if it goes wrong: rename `r8169.zst.bak` back, `depmod`,
`update-initramfs -u`.

### len-serv-003 is clevis/Tang bound — a reboot needs QUORUM
`/dev/nvme0n1p3`: `sss '{"t":2,"pins":{"tang":[.45,.46,.47]}}'`
**t=2 of 3, Tang-only, NO tpm2 pin.** Check quorum BEFORE any reboot:
```
for ip in 172.16.2.45 172.16.2.46 172.16.2.47; do
  curl -sS -m 6 -o /dev/null -w "$ip %{http_code}\n" "http://$ip/adv"; done
```
HTTP 200 (~993-byte JWS) = serving. Need ≥2. `tangd.socket` reporting `active`
is NOT sufficient — .45 has shown active while refusing :80.
**rpi-serv-001 (.45) flaps** — it has dropped both `/adv` and SSH while
reporting long uptime. It is the weak link.

### `clienttool` output is invisible without a pty
It checks `isatty()`. Redirected to a file it prints NOTHING (even `stdbuf -oL`
does not help), which looks identical to a driver failure. **Always run it
under a pty:**
```
sudo timeout --foreground -k 2 20 script -qec "./clienttool enp1s0f0" /dev/null
```
This confounded an early test and produced a false "the driver is broken"
conclusion.

### `pkill -f clienttool` kills your own shell
The pattern matches the ssh/bash command line containing the word. Use
`pkill -x clienttool`.

### Nuvoton TPM firmware update is KEY-DESTRUCTIVE
`nuvoton_tpm_fw_update_v2.0.zip` explicitly lists "ThinkCentre M715q-2, BIOS
M1XKT53A or above" — it applies to our hardware. It updates TPM 2.0 FW
7.2.0.1/7.2.0.2/7.2.1.0 → 7.2.2.0. **A TPM firmware update resets TPM-held
keys.** If any host ever gets a clevis `tpm2` pin or `systemd-cryptenroll
--tpm2`, flashing TPM FW makes it UNBOOTABLE. Check `clevis luks list` first.
Currently NO lenserv has a tpm2 binding, so it is presently safe — but verify.

---

## 10. The WinPE Work (built, boot-proven, currently unused for DASH)

Built on **windows-gpu** with the standard toolchain — `copype` → `DISM
/Add-Package` → `/Add-Driver` → `MakeWinPEMedia`.

- ADK + WinPE add-on **both pinned `10.1.26100.2454`** (versions MUST match;
  winget offers ADK `10.1.28000.1` which is a trap — the PE add-on only exists
  at 26100.2454).
- Optional components in Microsoft's required order: **WMI → NetFx → Scripting
  → PowerShell → {StorageWMI, DismCmdlets, SecureBootCmdlets}**, plus
  SecureStartup and EnhancedStorage, each followed by its `en-us` cab.
  `WinPE-SecureBootCmdlets` has **no** en-us cab.
- Realtek DASH NIC driver injected (`oem0.inf` rt640x64, `oem1.inf` rtvdevx64).
- `MakeWinPEMedia /ISO ... /bootex` → **PCA2023-signed** media. Without
  `/bootex` you get UEFI CA 2011 signing, which KB5025885 revokes.
- **VM GATE PASSED**: boots, `startnet.cmd` runs, NIC driver loads and DHCPs
  (hostname `minint-*`), WMI works, MAC gate fires, TLS with a **pinned**
  certificate completes, credential file downloads.
- Artifacts on U0: `/var/www/html/isos/WinPE-dash-amd64.iso` and
  `WinPE-dash-wow64.iso`. VM `winpe-dash-test` defined in libvirt.

**Why it cannot run DASHConfigRT:** the tool is x86 and **WinPE amd64 has no
WOW64**. Grafting `wow64.dll`/`wow64cpu.dll`/`wow64win.dll` into System32 was
tried and **verified to fail** — WOW64 needs kernel + registry support WinPE
does not carry. **Do not retry the graft.**
x86 WinPE is not an option either: WinPE for Win11 is x64/arm64 only (last
32-bit is the Win10 2004 add-on), UEFI x64 cannot boot an IA32 loader, and no
x86 Realtek DASH driver exists for MT 10VH.

### PXE serving bug — FIXED, but know the pattern
`/var/www/html/winpe/` had **no nginx location block**, so requests fell
through to `location /` and returned the Angular SPA `index.html` with
**HTTP 200** (11,256 bytes). iPXE would have loaded 11 KB of HTML as `boot.wim`
and failed deep in the Windows boot path. Fix script staged at
`/home/jdfalk/fix-winpe-pxe-paths.sh` on U0 — moves the payload under
`/isos/winpe/` (which IS served) and switches `menu.ipxe` to absolute URLs.
**Verify with a SIZE assertion, not just HTTP 200** — on a catch-all vhost,
200 does not mean the file exists.

Also note `${base-url}` is `http://172.16.2.30/ubuntu` and is only `set` INSIDE
the autoinstall/live labels — it is wrong/unset when jumping straight to
`:winpe`. Use absolute URLs.

---

## 11. Documentation Corrections Owed

These are WRONG in the repo and need fixing (use `changelog.d/` + `todo.d/`
fragments — do NOT hand-edit `CHANGELOG.md` or the `TODO.md` inbox):

1. **`todo.md:442-449`** and
   **`docs/agent-tasks/remote-power/TASK-02-amd-dash.md`** say DASH port
   **16992**. WRONG — measured **623/664**. 16992 is Intel AMT.
2. The same docs claim there is no Linux `DashDriver/autorun.sh` and that
   DASHConfigRT is Windows/WMI only. **WRONG** — a full official Linux toolkit
   exists (§5).
3. Notes describing len-serv-003 as plain ext4-on-LVM. It is ext4-on-LVM
   **on LUKS**.
4. Notes implying netbooting .96 unattended rebuilds the wrong layout are
   **too alarmist** — `menu.ipxe` sets `menu-default boot-local-disk` with a 5s
   timeout, so a stray PXE boot falls through to local disk. The stale
   PlainLuks seed at `/var/www/html/cloud-init/6c4b90bcf7f4/user-data` only
   fires if someone presses `i`.

---

## 12. Useful Discovered Resources

- **`download.lenovo.com/catalog/<MT>_Win10.xml`** and `_Win11.xml` — open,
  unauthenticated, machine-type keyed. Each package descriptor carries
  `<_PnPID>` so driver/hardware matches can be *verified* not assumed.
  **BUT the catalog is STALE** — for 10VH it lists BIOS M1XKT55A (2021) while
  M1XKT63A (2024) exists at a direct URL. Not authoritative.
- **`download.amd.com/software/`** — open, unauthenticated. AMC, DASH CLI,
  Provisioning Console, AIM-T all downloadable directly.
- **`pcsupport.lenovo.com` / `support.lenovo.com` 403 ALL scripted fetches.**
  So does `amd.com/en/developer`. **ASK THE OPERATOR to pull the file from a
  browser** rather than concluding it does not exist — this cost hours.
- Machine types found by probing: M75 Gen5 = 12RQ–12RX, 12SA–12SD, 12SH,
  12SK–12SM, 12ST–12SX. ThinkStation P8 = 30H0–30HX, 30J0–30JR.
- `ldiag_4.64.5_linux.iso` (Lenovo diagnostics live Debian, kernel 6.15.4) —
  **contains only in-tree `r8169`, no DASH tooling.** Dead end.
  Staged at `~/ai/lenovo-boot/` on U0 along with
  `lenovo_bootable_generator2.1.1.exe` (unexamined).

---

## 13. Absolute Paths — Where Everything Is

### On U0 (172.16.2.30)
```
/home/jdfalk/ai/p8iso/p8.iso                                  9.5 GB P8 driver ISO
/home/jdfalk/ai/p8iso/realtek/x/                              extracted Linux DASH toolkit
/home/jdfalk/ai/p8iso/ts_p8_dash_configuration_guide_v.10.pdf THE guide
/home/jdfalk/ai/p8iso/win/                                    P8 Windows Realtek pkgs (driver only)
/home/jdfalk/ai/lenovo-bios/m1xjy63usa.exe                    BIOS M1XKT63A
/home/jdfalk/ai/lenovo-bios/m1xct15usa.exe                    Super IO/EC firmware M1XCT15A
/home/jdfalk/ai/lenovo-tpm/x/                                 Nuvoton TPM FW updater (HAZARD §9)
/home/jdfalk/ai/lenovo-boot/                                  ldiag ISO + bootable generator
/home/jdfalk/dash-credentials-len-serv-003.txt                0600 — THE PASSWORD
/home/jdfalk/fix-winpe-pxe-paths.sh                           PXE path fix (run before PXE boot)
/home/jdfalk/add-winpe-menu-entry.sh                          already run
/home/jdfalk/finish-winpe-gate.sh                             VM gate helper
/var/www/html/isos/WinPE-dash-amd64.iso                       PCA2023-signed WinPE
/var/www/html/isos/WinPE-dash-wow64.iso                       + failed WOW64 graft
/var/www/html/ipxe/menu.ipxe                                  has a :winpe entry (needs path fix)
```

### On len-serv-003 (172.16.3.96)
```
/home/jdfalk/dashkit/DASHConfigRT/DASHConfigRT     Linux provisioning tool (static x86-64)
/home/jdfalk/dashkit/DASHConfigRT/config1.xml      real credentials, mode 600
/home/jdfalk/dashkit/ClientTool/clienttool         DASH client daemon
/home/jdfalk/dashkit/DashDriver/src/r8168.ko       built module
/usr/local/bin/clienttool                          installed
/usr/local/lib/dashclient/dash_client              service wrapper
/usr/lib/systemd/system/dash_client.service        enabled + active
```

### On windows-gpu (172.16.3.22)
```
C:\WinPE_amd64\                                                            WinPE working set
C:\dashwork\harvest\DASHConfigRT.exe                                       x86 Windows tool
C:\Program Files (x86)\AMD\Provisioning Console\Tools\DASHConfig-Realtek\  source of it
C:\Program Files\AMD\Manageability API\bin\dashcli.exe                     x64 DASH client CLI
```

---

## 14. Hard Truths / Anti-Patterns Learned

- **Verify the test before trusting the result.** A "the fleet is down" report
  came from broken SSH probes; the hosts had 1h28m uptime and had never
  rebooted. A "the driver is broken" conclusion came from stdout buffering.
- **`dumpbin /dependents` tells you imports, NOT architecture.** Check the PE
  machine type (`14C` = x86, `8664` = x64) or use `file`.
- **Fixtures are intent, not deployed state.** `enroll_tpm2: true` in
  `crates/uaa-core/tests/fixtures/components/len-serv-001.yaml` was never
  applied. Trust `clevis luks list` on the live host.
- **HTTP 200 does not mean the file exists** on a catch-all vhost.
- **When a vendor site 403s scripted fetches, ask the operator.** Do not
  conclude the artifact does not exist.

---

## 15. How to USE DASH Once Provisioned

Provisioning is only half the job. This is how you actually get remote power.

### Client tooling available
| Tool | Where | Arch | Notes |
|---|---|---|---|
| `dashcli.exe` | `C:\Program Files\AMD\Manageability API\bin\` on windows-gpu | **x64** | CLI, installed as an AMC dependency (AMD Manageability API v9.0.0.470) |
| AMD Management Console (AMC) | `C:\Program Files\AMD Management Console\` on windows-gpu | GUI | v14.0.0.1485, installed. `ui\AMCUI.exe` |
| `wsman` | len-serv-001 **only** | — | The openwsman CLI. Run cross-host FROM .92 |
| `AMD DASH CLI Setup_8.0.0.628.exe` | `download.amd.com/software/` | — | standalone CLI installer, downloaded to `C:\dashwork\amd\DASHCLI.exe` |

### AMC workflow (per the P8 guide §8)
1. Launch `AMCUI.exe` → **CONFIGURATION** → Authentication → Add Scheme.
   Auth Identifier `admin`, Scheme **Digest**, username/password from
   `config1.xml`.
2. Settings tab: Management Transport **HTTPS (preferred) 664**, HTTP 623.
   Tick **"Trust self signed certificate"** — DASH systems ship self-signed
   TLS, so without this discovery fails.
3. **HOME → DISCOVER** → enter IP (or hostname, or a TCP/IP range) → Next.
4. Expect: `'<ip>' is DASH capable`, Discovery Port **664**, DASH Version
   **1.2.0**, Product Vendor **Realtek**, Product Version = the NIC firmware
   version.
5. The system appears under "All Systems" with an inventory tree (BIOS version,
   boot config, DHCP client, IP interface, KVM redirection, memory, network
   port, processor, OS).

### Available operations (guide §9)
- **POWER** — power operation on a group or single system; select a power state
  and Apply. Shutdown, reboot, wake, sleep states.
- **BOOT** — including **BOOT TO BIOS** and **BOOT TEXT IMAGE**.
- **REDIRECTION** — text console (serial-over-LAN) and KVM.
  KVM viewers shipped with AMC: `AMDKVMViewer.exe`, `rtrdview.exe`,
  `tvnviewer.exe` (TightVNC), plus `putty.exe` and `ssh.exe` for text.
- **HEALTH** — sensor states (12 VCC, 12 VSB, 3 VCC, CPU, DIMM, M.2 …).
- **ALERTS**, **LOG ENTRY**, **FIRMWARE UPGRADE** (updates DASH firmware
  out-of-band — note this is the chicken-and-egg escape hatch, but requires
  DASH already working).

### Raw WS-MAN
Power management enumeration used during earlier probing:
```
wsman enumerate http://schemas.dmtf.org/wbem/wscim/1/cim-schema/2/CIM_AssociatedPowerManagementService \
  -h <ip> -P 664 -u <user> -p <pass> -y basic
```
**`wsman identify` is UNAUTHENTICATED and succeeds with any password.** It only
proves something is listening. Always use an authenticated verb to prove
credentials work.

### Verifying provisioning succeeded
```
ssh jdfalk@172.16.2.30 'nmap -Pn -p 623,664 172.16.3.96'
```
Both open = provisioned. Then confirm with an authenticated WS-MAN call or AMC
discovery — open ports alone do not prove the credentials took.

### Linux-side client service
`dash_client.service` must be running for the firmware to learn the host's
current IP. `clienttool` pushes IPv4/IPv6 into the OOB firmware and then
monitors for IP changes (`ip_change_monitor`). Without it the firmware may not
answer on the right address. Install/remove:
```
sudo /home/jdfalk/dashkit/ClientTool/install.sh          # install + enable + start
sudo /home/jdfalk/dashkit/ClientTool/install.sh /delete  # remove
```

---

## 16. The len-serv-001 / len-serv-002 Problem — Credentials Unknown

Both answer on 623/664, so DASH **is** provisioned and alive. Nobody knows the
credentials.

### What has been tried — ALL FAILED with 401
- `Administrator` / `Realtek`  (vendor default per Lenovo CDRT)
- `Administrator` / `Password` (the other documented vendor default)
- `Administrator` / `Admin`
Tested against .94 via
`wsman enumerate CIM_AssociatedPowerManagementService -y basic`.

### Why WinPE/DASHConfigRT does NOT rescue them
Per `DASHConfig_RT_guide.pdf`: reprovisioning a NIC that **already has**
credentials requires the OLD ones —
`DASHConfigRT -u:old_username -p:old_password -xf:config.xml`.
The Linux tool prompts interactively:
```
<Please confirm the existing account>
Input username:
Input password:
```
Bare `-xf:` only works when no password is set. So .92/.94 cannot be
reprovisioned without their current credentials.

### Remaining options for .92/.94
1. **BIOS "Reset DASH Credentials".** The Lenovo CDRT AMD DASH guide states
   credentials can be reset under **RealManage Setup** in BIOS. This
   CONTRADICTS the measurement that the M715q BIOS exposes only
   `DASH Support = Disabled;Enabled` with no credential submenu (confirmed by
   the operator at the console, and by ThinkLMI which shows 75 attributes and
   only that one DASH entry). RealManage config on Realtek platforms is often
   a **separate POST hotkey**, not a main-menu item. **UNVERIFIED — worth
   watching POST on .92 for a RealManage/Ctrl-key prompt.**
2. **Physical labels.** Chassis and pull-out tab were photographed. Only
   MT-M `10VH-S1X800` and S/N `MJ0AAHEE` found. A handwritten label was
   re-photographed at higher quality and rotated — it is NOT credentials.
3. **Third-party:** `github.com/88plug/realtek-realmanage` (`dash-activate`).
   UNVETTED. Our RTL8111EPP `[10ec:8168]` is in its "should work" tier, not its
   tested tier (its primaries are RTL8125BP/8127AP). **Not run. Needs an
   explicit go — it writes NIC firmware and a bad write could kill the NIC.**
4. **Accept and move on.** .92/.94 are byte-identity protected until migration
   waves 7-10 anyway. They are not scheduled for a wipe.

### Important
Whatever is learned on .96 does NOT transfer to .92/.94 — the blocker there is
authentication, not tooling.

---

## 17. Session Timeline (what was attempted, in order, and why it failed)

Useful so a fresh agent does not repeat any of it.

1. **Built WinPE** on windows-gpu with copype/DISM/MakeWinPEMedia. Correct,
   boot-proven, VM gate passed. Not usable for DASHConfigRT (x86 vs no WOW64).
2. **Grafted WOW64 DLLs** (`wow64.dll`, `wow64cpu.dll`, `wow64win.dll`,
   `wow64base.dll`, `wow64con.dll` at 10.0.26100.8875) into the WIM's System32,
   rebuilt the ISO, verified the DLLs were present and the VM booted that exact
   ISO. **Still failed identically.** WOW64 needs kernel + registry support.
   **DEAD END — do not retry.**
3. **Hunted for an x64 DASHConfigRT** — AMD Provisioning Console, AMD
   Management Console, AMD DASH CLI, Lenovo catalogs for M75 Gen5 and P8, and a
   user-supplied Lenovo package. **All x86. Byte-identical hashes.** No x64
   build exists.
4. **Considered x86 WinPE** — blocked three ways: WinPE for Win11 is x64/arm64
   only, UEFI x64 cannot boot an IA32 loader, and no x86 Realtek DASH driver
   exists for MT 10VH.
5. **Checked for an existing Windows install on .96** to run the x86 tool —
   none; the NVRAM Windows entry is stale.
6. **Found the Linux toolkit** in the P8 driver ISO. Built r8168 against kernel
   7.0.0-28 — **succeeded**. DASH firmware became reachable.
7. **Ran DASHConfigRT** → failed, `Open write pipe failed 2` (no FIFO).
8. **Installed dash_client.service** so the FIFO would exist → service runs,
   but `clienttool` stalls and never creates `/tmp/DasH`.
9. **Rebooted .96 clean** (r8168 loads from initramfs, clevis unlocked,
   dash_client auto-started) → **same stall**. Reboot did not help.
10. **Ruled out** BIOS DASH setting, BIOS version, orphan processes,
    PrivateTmp, net-tools, IPv6, and a NIC firmware update package existing.

**Never attempted:** strace of the stalled clienttool. That is the next step.
