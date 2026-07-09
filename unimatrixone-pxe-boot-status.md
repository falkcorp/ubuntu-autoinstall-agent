<!-- file: unimatrixone-pxe-boot-status.md -->
<!-- version: 6.0.0 -->
<!-- guid: 8f3a1c72-6d4e-4b9a-9e21-7c5b0a2f4d18 -->
<!-- last-edited: 2026-07-08 -->

# unimatrixone PXE Boot Debug — Status (2026-07-08 evening — READ THIS SECTION FIRST)

## RESUME HERE NEXT SESSION

**Everything below "## HISTORICAL (2026-07-07 night)" is superseded** — kept for reference only.

### What happened 2026-07-08

1. Built `scripts/capture-uni-boot.sh` — a single-command diagnostic wrapper on the server
   that runs `tcpdump` (full pcap, `ether host ac:1f:6b:40:fc:e2 or icmp6` filter), a dnsmasq
   journal tail, and an nginx `access.log` tail together, all under `nohup` so a dropped SSH
   session (which lost the plain `journalctl -f` tail on 2026-07-07 night) can no longer
   silently lose the capture. Output lands in `~/uni-boot-capture/` on 172.16.2.30, timestamped,
   with a Ctrl+C summary. (First version used `disown` instead of `nohup` and had the same
   drop risk — fixed to `nohup` at 14:40.)
2. Ran a capture attempt (~12:33) — again **zero packets and zero dnsmasq/nginx log entries**
   for unimatrixone, this time captured with the more robust tooling, reinforcing that this
   isn't just a monitoring gap.
3. Powered unimatrixone on again at 14:34 EDT (`chassis bootdev pxe options=efiboot` + power
   on) and investigated further.
4. **Found a likely root cause distinct from the IPv6/DHCPv6 config**: downloaded the X10DSC+
   manual via the Wayback Machine (board is EOL, off Supermicro's site) and diagnosed that the
   **Intel 82599 add-in NIC's Option ROM is disabled per-slot** in
   `Advanced → PCIe/PCI/PnP Configuration` — a *different* setting from `Ipv6 PXE Support`
   (already re-enabled on 2026-07-06), also reset by the CMOS clear. Without the slot's Option
   ROM enabled, the card's PXE stack (IPv4 or IPv6) may never initialize at all, which would
   fully explain zero server-side traffic regardless of the netboot server's IPv6 config being
   correct. Identified fixes: re-enable the 82599's per-slot Option ROM, and re-verify
   `UEFI NETWORK Drive BBS Priorities`.
5. This evening, went back into BIOS to apply the above and tested disabling CSM
   (Compatibility Support Module) along the way — **this locked the machine at "Discovering
   PCIe devices"**, the same hang seen once before. Had to CMOS-clear via the JBT1 jumper
   again, which means `Ipv6 PXE Support` AND the 82599 per-slot Option ROM setting (step 4)
   are now BOTH back to defaults and need to be reapplied from scratch next session, along
   with re-deciding CSM on/off (leave enabled — it's what caused tonight's lockup).
6. **New theory raised by the user, not yet investigated**: the actual root cause may be
   **IPMI / the IPMI NIC configuration**, not the host BIOS or netboot server config. Flagged
   as the top open thread for next session — see "Next steps" below.
7. Powered unimatrixone off cleanly for the night via IPMI (`chassis power off`, confirmed
   `Chassis Power is off` after a short settle delay). Note: local macOS `ipmitool` was
   crashing with a `SIGPIPE`/exit-141 on every call tonight (cause not diagnosed) — power-off
   was sent successfully by running the same `ipmitool` command over SSH on 172.16.2.30
   instead, which worked normally. Worth using the server as the IPMI control point by default
   until the local crash is understood.

### Next steps

1. Investigate the new IPMI/IPMI-NIC theory: is the IPMI NIC genuinely dedicated/separate from
   the 82599 data NIC as documented in "The machine" below (reconfirm — don't assume it hasn't
   changed), check IPMI LAN channel config (`ipmitool lan print 1`), and consider whether any
   BMC-side network setting (shared NIC/failover mode, VLAN tagging on the IPMI channel, etc.)
   could interfere with the host's own PXE/network stack.
2. Once back in BIOS after tonight's CMOS clear: re-enable `Ipv6 PXE Support` AND the 82599's
   per-slot Option ROM (both in `Advanced → PCIe/PCI/PnP Configuration`), re-verify
   `UEFI NETWORK Drive BBS Priorities`, and leave CSM enabled unless there's a specific reason
   to retest disabling it.
3. Re-run the IPv6 PXE attempt using `scripts/capture-uni-boot.sh` (nohup-safe now) for a
   gap-free pcap + log capture, with the user watching their own SOL session directly, to
   determine whether the per-slot Option ROM fix (most likely explanation as of today) resolves
   the zero-server-side-traffic result.
4. The original 2026-07-07 discrepancy (server logs showed nothing, but the user's console
   showed progress past the IPv6 PXE stage with a delay) is still unresolved and should be
   revisited once the Option ROM fix is in place and the attempt is reproducible.

---

## HISTORICAL (2026-07-07 night) — superseded, kept for reference only

### What happened 2026-07-07 night

1. Applied `~/setup-ipv6-netboot.sh` on the server (172.16.2.30) at 21:14 — confirmed clean:
   `radvd` active + enabled, `dnsmasq` active, ULA `fd37:59dd:a92e:a257::1/64` present on
   `enp8s0f0`, and dnsmasq's own log confirms the pool loaded:
   `DHCPv6, IP range fd37:59dd:a92e:a257::100 -- ::1ff, lease time 12h`.
2. Power-cycled unimatrixone via IPMI (`chassis bootdev pxe options=efiboot` +
   `chassis power on`) at 21:16.
3. Monitored server-side for the boot attempt (`journalctl -u dnsmasq -f`, `nginx`
   `access.log`) for 8+ minutes: **zero packets/log entries ever attributed to
   unimatrixone's MAC (`ac:1f:6b:40:fc:e2`)** — no DHCPv6 solicit/advertise, no arch-16
   HTTPClient request, no nginx hit for `grubnetx64.efi` or `casper/*`. Only traffic seen
   in that window was an unrelated device ("Watch") doing routine IPv4 DHCP renewals.
4. **However, the user — watching the actual console/SOL directly — reports it got past
   the IPv6 PXE stage and was "delaying there a while"** before having to step away. This
   directly conflicts with the server-side log evidence above. See "Open discrepancy"
   below — resolve this first next session.
5. Had to shut down for the night due to fan noise. Powered off cleanly via IPMI
   (`chassis power off`), confirmed `System Power: off` at 21:22.

### Open discrepancy — resolve this first

Server-side logs show no evidence unimatrixone's IPv6 PXE attempt ever reached dnsmasq or
nginx, but the user's own console observation says it visibly progressed past the IPv6 PXE
step. Possible explanations to check next session, in rough priority order:

- The one dnsmasq journal tail session had a gap: its SSH connection silently dropped
  around 21:19–21:21 and had to be restarted. A request could have landed in that ~90s
  window and simply never been captured. Re-check with a precise `journalctl --since
  "<power-on timestamp>"` once the exact power-on time is known, or better —
- Capture the *whole* attempt with `tcpdump -i enp8s0f0 -w /tmp/uni-ipv6-boot.pcap 'icmp6
  or port 546 or port 547'` (a pcap file survives an SSH drop; a piped `-f` journal tail
  does not) so nothing can be missed regardless of connection hiccups.
- Firmware's IPv6 PXE client may resolve the boot file via a different mechanism than the
  DHCPv6 option-61/59 path this config assumes — worth capturing raw wire traffic
  independent of what dnsmasq chooses to log, not just trusting dnsmasq's own log lines.
- The firmware might just display/parse an "IPv6 PXE" stage in its own boot-menu UI
  (visually looking like "past IPv6 PXE") without ever actually issuing a real DHCPv6
  request the server could see — i.e. an attempt/banner on screen, not a completed
  handshake. Get an exact description or screenshot of what's on screen at the "delay"
  point next time to settle this quickly.

### Next steps

1. Power on unimatrixone (`chassis bootdev pxe options=efiboot` + `chassis power on`) —
   the IPv6 netboot config is already applied and confirmed live server-side; no need to
   re-run `setup-ipv6-netboot.sh`.
2. Capture with an unbroken `tcpdump -i enp8s0f0 -w /tmp/uni-ipv6-boot.pcap 'icmp6 or port
   546 or port 547'` for the entire attempt, run in parallel with (not instead of) the
   dnsmasq journal tail.
3. Have the user watch the console directly via their own SOL session (established in an
   earlier session that Claude's own SOL connections interfere with the shared BMC video
   console relay — do not reconnect SOL from this side) and report exactly what's on
   screen at the point it "delays."
4. If it's genuinely stuck/delaying at the IPv6 PXE step with no observable network
   activity: firmware's `Ipv6 PXE Support` setting was already confirmed enabled on
   2026-07-06, so re-verify it's still set (a future CMOS clear could reset it again).
   Also consider whether IPv4 (still stuck at `PXE-E18` per the 2026-07-02 diagnosis — the
   VLAN/switch-port root cause was never actually fixed) should be raced in parallel, or
   whether to fall back to the always-available proven-bootable Ubuntu 26.04 live-server
   USB stick instead of continuing to chase netboot.

---

## HISTORICAL (2026-07-06) — superseded, kept for reference only

**Everything below "TL;DR (current — root cause is the SWITCH PORT...)" is from 2026-07-02
and was already superseded as of 2026-07-06** — a lot had changed since (0x91 boot hang
fixed, a CMOS clear reset all BIOS settings, and the VLAN-trunk diagnosis was no longer the
active theory as of that date). Kept for historical reference only.

### What happened 2026-07-06

1. **0x91 UEFI boot hang is FIXED** — root cause was the RAID/BBU sensor flapping during PCI
   enumeration (see historical section further down); fixed via a full JBT1 CMOS-clear jumper
   procedure (not just a battery pull). Machine boots normally again.
2. **The CMOS clear reset ALL BIOS settings to factory defaults**, which broke netboot that
   had been proven fully working end-to-end on 2026-07-03. This is the source of everything
   else investigated today.
3. Ruled out today: switch/VLAN issue (a live Ubuntu USB boot gets a real DHCP lease fine,
   seen by the server), and PXE protocol priority order (reordered IPv4-before-IPv6 in
   "UEFI NETWORK Drive BBS Priorities", saved, retested — no change, still `PXE-E18`, zero
   packets ever seen by the server's dnsmasq for the IPv4 attempt).
4. **Found the real culprit**: `Ipv6 PXE Support` was sitting **Disabled** in
   `Advanced → PCIe/PCI/PnP Configuration` (scroll to the very bottom, past the per-slot
   Option ROM lines — it's grouped with `Network Stack` and `Ipv4 PXE Support`, not under
   Boot Feature or the NIC's own config page, which is where we'd searched before). User
   confirms it *was* previously enabled (before the CMOS clear) — this is almost certainly
   the actual missing piece, not a network/server-side issue at all.
5. Built out full IPv6 network infrastructure on the server (172.16.2.30) in preparation:
   - `radvd` config (RA with `AdvManagedFlag on` — stateful DHCPv6 required, most UEFI
     firmware won't netboot over IPv6 on SLAAC alone — but `AdvDefaultLifetime 0` so this
     network can **never** become anyone's default IPv6 gateway; deliberate, because
     172.16.2.30 has no real upstream IPv6 path and is double-NAT'd behind the UDM Pro).
   - `dnsmasq` DHCPv6 stanza: stateful address pool + `option6:61` (client-arch-type) match
     for arch 16 → `option6:bootfile-url` pointing at `grubnetx64.efi` over the ULA address.
   - ULA prefix used: `fd37:59dd:a92e:a257::/64` (server gets `::1`, pool is `::100`-`::1ff`).
   - **Script written, applied the following night (2026-07-07)**: `~/setup-ipv6-netboot.sh`
     on 172.16.2.30. See the 2026-07-07 section above for the apply + boot-attempt result.
6. Confirmed nginx already listens dual-stack (`[::]:80`/`[::]:443`) — no nginx changes needed.
7. Both target SSDs (`sda`/`sdb`) are fully wiped (`wipefs`, `sgdisk --zap-all`, zeroed) —
   ready for install whenever needed, netboot or not.
8. A working, proven-bootable Ubuntu 26.04 live-server USB stick exists (GPT+ESP partitioned —
   a bare unpartitioned FAT32 image does NOT boot on this board's BIOS) with confirmed-working
   native networking (`ixgbe` driver gets a real DHCP lease). **Always-available fallback**:
   install directly from this USB, bypassing firmware netboot entirely, if IPv6 doesn't pan out.
9. Machine was powered off at the end of the 2026-07-06 session (per explicit user instruction).

### Useful reference from 2026-07-06

- IPMI: `ipmitool -I lanplus -H 172.16.3.150 -U ADMIN -P ADMIN <cmd>` (run via ssh to
  172.16.2.30 as `jdfalk`).
- SOL reconnect pattern (needed repeatedly — single-payload BMC limitation, sessions get
  bumped): `sol deactivate` then `sol activate`; for scripted keystroke injection use a FIFO
  with a persistent open writer:
  ```bash
  mkfifo /tmp/sol_local.fifo
  exec 3<>/tmp/sol_local.fifo
  cat <&3 | ssh -tt jdfalk@172.16.2.30 'ipmitool -I lanplus -H 172.16.3.150 -U ADMIN -P ADMIN sol activate' > sol.log 2>&1 &
  # then: printf '<keys>' > /tmp/sol_local.fifo
  ```
  The `-tt` (force pty) on the ssh command is required — without it, ipmitool's sol activate
  immediately fails with `tcgetattr: Inappropriate ioctl for device`.
- Server non-interactive `sudo` does not work over SSH (password required) — any command
  needing root has to be handed to the user to run themselves, or scripted and left for them
  to `sudo bash` (as done with `~/setup-ipv6-netboot.sh`). Plain `journalctl` (no sudo) works
  fine for `dnsmasq`/`nginx`/`tftpd` since `jdfalk` is in the `adm` group.
- **Important lesson learned 2026-07-06**: racing the DEL keypress over SOL during the
  early POST/logo screen does **not** work on this board — tried both the ANSI Delete
  sequence (`\x1b[3~`) and the raw DEL byte (`0x7f`), neither registered, and a bare
  printable character also produced zero effect, confirming SOL keyboard input is simply
  not accepted at that POST phase. `chassis bootdev bios` forces entry into Setup
  automatically once POST completes, no keypress needed — use that instead.

---

## HISTORICAL (2026-07-02) — superseded, kept for reference only

## TL;DR (current — root cause is the SWITCH PORT, not the NIC/driver)

**Correction to earlier "dead end" conclusion**: iPXE/GRUB driver defects were the wrong
diagnosis. The actual root cause is that unimatrixone's switch port passes traffic for
**multiple VLANs** (confirmed by the user) instead of being a single-VLAN access port.
Two independent netboot network stacks (iPXE and GRUB) both fail identically — DHCP never
returns a usable IPv4 address — while raw UEFI firmware's own boot-time HTTP client always
succeeds. This is the signature of VLAN-tagged frames reaching the NIC that a strict
netboot stack rejects, while firmware's own client doesn't do that check.

**The fix (confirmed necessary, not attempted yet as of this writing)**: restrict the
switch port to a single access VLAN matching `172.16.2.0/23` in UniFi. User has confirmed
they can do this. Everything else (dnsmasq target, GRUB standalone binary with auto
network bring-up, casper images) is staged and should work the moment the port is fixed —
no further per-boot intervention needed.

## Evidence for the VLAN diagnosis

1. **iPXE, NII driver**: `dhcp` → "No configuration methods succeeded"; `ifstat` shows
   14,000+ RXE errors. Decoded iPXE error `0x42306095` (dominant, 2375 occurrences) =
   **"packet received for a VLAN that is not defined in iPXE"** (source: `net/vlan.c:258`).
2. **iPXE, SNP driver** (forced via patch — see below): `ifstat` clean at idle (RXE:0),
   `dhcp` returns "ok" but only IPv6/SLAAC (no IPv4). Static IPv4 + SNP → HTTP "Connection
   timed out"; a `ping` reveals **TXE: 12,193,363** — SNP collapses under real traffic
   exactly like NII once traffic flows.
3. **GRUB standalone netboot, DHCP** (completely independent codebase/stack): `net_ls_cards`
   detects the NIC fine; `net_add_addr net0 efinet0 dhcp` returns address `0.0.0.0` — same
   DHCP failure as iPXE, via unrelated code.
4. **GRUB standalone netboot, STATIC IP** (rules out DHCP-specific bugs): with correct
   syntax (`net_add_addr net0 efinet0 172.16.2.35` + `net_add_route default 0.0.0.0/0 gw
   172.16.2.1`), the address AND route configure correctly (`net_ls_addr`/`net_ls_routes`
   confirm it) — but the kernel fetch then fails with **"couldn't send network packet"**,
   which is a transmit-level failure consistent with **ARP resolution for the gateway
   never completing** (GRUB has an IP for 172.16.2.1 but never learns its MAC). ARP is the
   most basic possible IPv4 mechanism — if even ARP replies are being rejected/lost, no
   higher-layer protocol can work regardless of address configuration.
5. **UEFI firmware's own HTTP client** (used to download snponly.efi / grubnetx64.efi) has
   NEVER failed — always clean 200 downloads, no errors, regardless of file size.
6. **User confirmation**: the port is "on the right VLAN [access] but allows traffic for
   all of them" — i.e., it's a trunk carrying additional tagged VLANs beyond the native one.

**VLAN sub-interface workaround attempted and characterized (not viable)**: rebuilt iPXE
with `DEBUG=vlan:2` to see exactly which VLAN tags hit the port. Result:
**VLANs 1, 10, and 500** are all present. Testing DHCP through `vcreate --tag`-created
sub-interfaces: VLAN 1 sometimes got a real DHCP `ok` (the only tag that ever worked; 10
and 500 always failed outright with "No configuration methods succeeded"), but repeat
testing showed VLAN 1's success was **flaky, not reliable** — a subsequent identical test
failed the same way, and a third attempt saw the entire 280-second capture window consumed
by VLAN 10 broadcast flood before DHCP could even be attempted (3557 log lines, effectively
100% VLAN-10 noise). This proves the flood volume is a genuine broadcast-storm level, not
occasional interference — creating a tagged sub-interface only filters frames in *software*
after they've already consumed the shared NIC receive ring; it does not reduce the volume
hitting the hardware, so legitimate traffic can still be crowded out unpredictably. No
software-only fix is viable here; the switch fix (removing VLANs 10/500 from this port
entirely) is not just recommended but **necessary** to get reliable behavior. Cleaned up:
`snponly.efi` rebuilt without the debug flag (its own print overhead worsened timing) and
with the stock `dhcp` + `chain boot.ipxe` script, ready for whenever the port is fixed.

**Also tested and ruled out: forced-SNP + pure `autoboot`** (no embedded script). Hypothesis
was that `autoboot` might inherit UEFI's already-negotiated network state and skip a fresh
DHCP/ARP negotiation, shortening the window exposed to the VLAN 10/500 broadcast storm.
DISPROVEN: `autoboot` still runs its own fresh DHCP from scratch (it only differs in what
it attempts *after* — chaining to a default bootfile — not in whether renegotiation
happens). Result was identical to every other attempt (`Configuring ... ok` but IPv6-only,
no IPv4) — plus a NEW data point: **TXE: 30 transmit errors** this time, showing the storm
corrupts outbound packets too, not just inbound. Then `Nothing to boot` since there's no
IPv4 route. `snponly.efi` restored to the clean stock build afterward (SNP patch reverted
back to `.orig`, embedded script back to plain `dhcp` + `chain boot.ipxe`).

## Exhaustive summary — every software avenue tried, none viable

| Approach | Result |
|---|---|
| iPXE NII, `dhcp` | 0.0.0.0, "No configuration methods succeeded" |
| iPXE SNP (forced), `dhcp` | IPv6-only "ok", no IPv4 |
| iPXE SNP + static IP | Route configures, HTTP "Connection timed out", TXE 12M under load |
| iPXE SNP + `autoboot` (no script) | Same as SNP+dhcp: IPv6-only, now also TXE errors |
| GRUB standalone, DHCP | 0.0.0.0 |
| GRUB standalone, static IP + route | Address/route configure correctly, ARP fails ("couldn't send network packet") |
| GRUB static IP, repeat | Same failure, confirms not transient |
| VLAN sub-interfaces (1/10/500) via `vcreate` | VLAN 1 DHCP flaky (works once, fails twice more); VLANs 10/500 always fail; one capture window was 100% VLAN-10 broadcast noise (3557 lines) with no room for DHCP at all |
| Unified Kernel Image (considered) | Rejected without building — wouldn't route around the defect since plain Linux (no 8021q module) likely hits the same tagged-frame rejection |
| Firmware update | None exists; BIOS/BMC already at latest for this EOL board |

**Conclusion is now airtight, and the mechanism is cleaner than "storm timing luck."** A
follow-up clean re-test of the VLAN 1 sub-interface (the only tag that ever showed a
positive DHCP `ok`) FAILED outright on retest — even IPv6 fell back to link-local-only,
`(inaccessible)`. There is **zero confirmed evidence any interface, tagged or untagged, has
ever received genuine IPv4** today; the earlier VLAN-1 "ok" was never actually verified as
real IPv4 (blocked by a hyphen-in-variable-name iPXE parsing bug at the time) and, in
hindsight, was almost certainly IPv6 SLAAC success — the SAME thing plain `net0` gets every
time. **The unifying pattern across every single test today**: IPv6 SLAAC (multicast Router
Advertisement, needs no unicast reply routed back to this specific port) succeeds
regularly; anything requiring a **unicast** reply delivered specifically back to this port
— DHCPv4 ACK, ARP reply, TCP SYN-ACK — has never once succeeded, regardless of driver,
bootloader, or VLAN tag used. This is not a driver, script, or configuration bug on our
side, and it is not solvable by out-racing broadcast noise. Restricting the switch port to
a single VLAN (removing 10/500) is the only remaining fix, and it is external — no further
investigation from this end will change that.

## Alternate/additional switch-side hypothesis worth checking: stale MAC table entry

The "multicast always works, unicast never does" pattern is *equally* consistent with a
**stale CAM (MAC address table) entry** for `ac:1f:6b:40:fc:e2` pointing at the wrong port,
independent of (or in addition to) the VLAN trunk issue. If the switch learned this MAC on
a different port at some point — plausible given dozens of rapid link up/down cycles from
today's repeated IPMI power resets — unicast replies would be forwarded to the stale
location and never reach unimatrixone, while broadcast/multicast (IPv6 RA) floods to all
ports regardless of CAM state and would still arrive. **Ask whoever has switch access to
also check/clear the MAC address table entry for `ac:1f:6b:40:fc:e2`** (most managed
switches: `clear mac address-table dynamic address <mac>`, or the UniFi-equivalent) in
addition to the VLAN restriction — the fix might be one, the other, or both.

**UPDATE — stale-MAC theory DISPROVEN.** Tested with a completely fresh, never-before-seen
spoofed MAC (`02:00:00:12:34:56`, set via `set net0/mac` before `ifopen`) — a switch could
have zero prior CAM history for this address. Result: **identical failure pattern**
(`Configuring ... ok` but IPv6-only, no real IPv4) as every test using the real MAC. This
conclusively rules out MAC-table staleness — the issue is **port-wide**, not tied to any
specific MAC's switch-learned history. Whoever fixes the switch does not need to worry
about clearing CAM entries; the VLAN/trunk (or possibly a port-level feature like DHCP
Snooping trust settings) is the only remaining candidate.

**Also checked and ruled out: booting via IPv6 instead of IPv4.** Since IPv6 SLAAC reliably
succeeds (unlike IPv4), considered fetching boot resources over IPv6 (which uses Neighbor
Discovery Protocol instead of ARP — a different L2 resolution mechanism that might not hit
whatever blocks IPv4 unicast). Ruled out by inspection, no hardware test needed: the server
(172.16.2.30) has **no IPv6 address in the `fde2:e44:a227:4a0e::/64` prefix** unimatrixone
receives via SLAAC on any interface — so even a fully working IPv6 config on unimatrixone
couldn't reach the server anyway. Not a viable path regardless of NDP behavior.

**Context for whoever fixes the switch**: the server itself has a `vlan10@enp8s0f0`
interface, confirming **VLAN 10 is a real, intentionally-used network elsewhere on this
infrastructure** — not just noise. The fix should scope unimatrixone's *specific port* to
exclude VLANs 10/500 (single access VLAN for `172.16.2.0/23`), not remove VLAN 10 from the
network entirely.

## Prioritized checklist for whoever has UniFi switch access

Given the confirmed symptom (multicast always works, unicast reply delivery never does,
port-wide and MAC-independent), try in this order:

1. **Restrict the port to a single access VLAN** matching `172.16.2.0/23` (primary theory,
   already asked for). Remove VLANs 10/500 from this specific port only — they're real,
   used networks elsewhere (the server itself has a `vlan10` interface).
2. **Check Storm Control settings** on this port (UniFi: Settings → Networks/Switch Ports →
   Storm Control). If broadcast/multicast/unknown-unicast limiting is aggressive, it could
   explain "some multicast gets through, unicast replies get dropped" independent of VLAN
   config — unknown-unicast (destination MAC not yet learned by the switch) is sometimes
   rate-limited more aggressively than broadcast/multicast.
3. **Check DHCP Snooping / trust settings** for this port, in case unicast DHCP/ARP-adjacent
   traffic is being filtered by a security feature rather than dropped for VLAN reasons.
4. MAC address table clearing — **already tested and ruled out** (fresh spoofed MAC fails
   identically), no need to check this.

**Also tried and ruled out: forcing DHCP's broadcast-reply flag.** iPXE only sets
`BOOTP_FL_BROADCAST` (a hint asking the DHCP server to reply via L2 broadcast instead of
unicast) when the interface already has an IPv4 address (i.e. on renewal) —
`src/net/udp/dhcp.c`, condition `ipv4_has_any_addr(netdev)`. For the very first negotiation
(our exact failure case) it defaults to expecting unicast. Patched the condition to force
the flag unconditionally and rebuilt — **no change**, identical IPv6-only `ok` result.
This confirms the flag is only a *request*; the DHCP server (dnsmasq proxy + UniFi) decides
independently whether to honor it, and evidently doesn't here. Not a client-side-fixable
lever. Patch reverted, stock `net/udp/dhcp.c` restored.

**Minor aside, not pursued further**: tftpd-hpa has zero log entries (not even error-level —
it does log some errors without `--verbose`, confirmed via one old unrelated crash entry) for
any of today's dozens of arch-7 `PXE-E21: Remote boot cancelled` failures. This *could*
indicate the legacy TFTP path fails for a reason unrelated to VLAN tagging (e.g. a PXE-ROM
quirk with proxy-DHCP mode specific to old-style TFTP boot, distinct from the newer arch-16
HTTP Boot spec this board handles fine). Not investigated further — would need more sudo
scope (`--verbose` + service restart) for uncertain payoff, and it's much weaker evidence
than the protocol-level diagnostics above. Doesn't change the diagnosis or required fix.

Conclusion: FOUR independent tests (iPXE×2 drivers, GRUB×2 addressing modes) all fail at
exactly the layer where a switch-side tagged-frame rejection would predict — including the
most basic possible mechanism (ARP) — while firmware's own client, which doesn't do
VLAN-tag validation, always gets through cleanly. This is not a software/driver bug; there
is no further software-only workaround to try. The switch port fix is the only remaining
path.

**Considered and rejected: Unified Kernel Image (UKI) to skip GRUB/iPXE entirely.** Idea
was to `objcopy` vmlinuz+initrd+cmdline into one PE binary so UEFI HTTP Boot executes it
directly with zero further netboot-stack involvement. Rejected because it wouldn't actually
route around the defect: earlier "the 82599 works fine under Linux" was never actually
verified over *this* switch port (SSH to the local Ubuntu 24.10 install failed with "No
route to host"). Without the `8021q` module loaded, plain Linux networking drops
802.1Q-tagged frames for unregistered VLANs much like iPXE/GRUB do — so a UKI would likely
just move the identical DHCP/ARP failure somewhere far harder to diagnose (no
`net_ls_addr`-style introspection inside a minimal live boot). Not worth the risk of a
corrupted/hanging hand-built EFI image costing multiple blind POST cycles to debug.

## What's staged and ready (fires automatically once the port is fixed)

- **dnsmasq arch-16 target** → `http://172.16.2.30/ipxe/grub/grubnetx64.efi` (a standalone,
  self-contained GRUB EFI build — no manual typing needed, no prefix-detection required).
- **The GRUB binary** is built via `grub-mkstandalone` with an **embedded** `grub.cfg`
  (baked in as `(memdisk)/boot/grub/grub.cfg`, auto-executes on load — same technique as
  iPXE's `EMBED=`). Current embedded script (`/tmp/grub-standalone.cfg` on the server):
  ```
  insmod efinet
  insmod net
  insmod http
  set timeout=3
  set pager=0
  echo "GRUB standalone netboot v2 - unimatrixone"
  net_ls_cards
  net_add_addr net0 efinet0 dhcp
  net_ls_addr
  net_ls_routes
  linux (http,172.16.2.30)/ubuntu/casper/vmlinuz boot=casper root=/dev/ram0 \
    ramdisk_size=1500000 ip=dhcp netboot=http \
    iso-url=http://172.16.2.30/isos/ubuntu-26.04-live-server-amd64.iso
  initrd (http,172.16.2.30)/ubuntu/casper/initrd
  boot
  ```
  Rebuild recipe (no sudo needed, files are ACL-writable):
  ```
  grub-mkstandalone -O x86_64-efi -o /tmp/grubnet_standalone.efi \
    --modules='normal linux configfile echo test true sleep halt reboot efinet net http tftp search' \
    'boot/grub/grub.cfg=/tmp/grub-standalone.cfg'
  cp /tmp/grubnet_standalone.efi /var/www/html/ipxe/grub/grubnetx64.efi
  ```
- **Passwordless sudo helper** installed at `/usr/local/sbin/unimatrixone-netboot-set.sh`
  (sudoers drop-in `/etc/sudoers.d/99-unimatrixone-netboot`). Only 3 exact invocations are
  whitelisted (no free-form args reach root):
  ```
  sudo /usr/local/sbin/unimatrixone-netboot-set.sh snponly   # point at iPXE snponly.efi
  sudo /usr/local/sbin/unimatrixone-netboot-set.sh grub      # point at GRUB (current)
  sudo /usr/local/sbin/unimatrixone-netboot-set.sh disabled  # comment out arch-16 entirely
  ```
- **Casper kernel/initrd/ISO** all verified serving 200 over HTTP at `/ubuntu/casper/` and
  `/isos/`.
- **mac file** `/var/www/html/ipxe/boot/mac-ac1f6b40fce2.ipxe` defaults to `live` boot
  (Ubuntu 26.04 live/casper) via the original iPXE chain — not currently in use since we're
  on the GRUB path, but restored to stock and ready if we go back to iPXE.

## Next step (once VLAN port fix lands)

Just IPMI power-reset unimatrixone with PXE+EFI boot flag set:
```
ipmitool -I lanplus -H 172.16.3.150 -U ADMIN -P ADMIN chassis bootdev pxe options=efiboot
ipmitool -I lanplus -H 172.16.3.150 -U ADMIN -P ADMIN chassis power reset
```
Watch nginx (`/var/log/nginx/access.log`) for the chain: `grubnetx64.efi` →
`ubuntu/casper/vmlinuz` → `ubuntu/casper/initrd`. SOL capture via
`ipmitool ... sol activate` (deactivate first) will show the embedded script's echo/net
diagnostics if anything still fails.

## After it boots into live (the actual goal)

1. SSH into the live environment (network should come up via `ip=dhcp` in the casper
   kernel cmdline, using the OS's native ixgbe driver post-boot — no netboot-stack
   involvement at that point) → disk inventory:
   `lsblk -o NAME,SIZE,TYPE,FSTYPE,MOUNTPOINT`, `cat /proc/mdstat`, `lspci | grep -i raid`
2. Decide storage layout (direct vs raid1) — machine has an existing Ubuntu 24.10 install
   on local disk (found when a CMOS reset flipped BIOS to Legacy mode and it fell through
   to local disk — unreachable at .2.35, no known creds).
3. Approve in agent registry: `curl http://172.16.2.30:25000/api/approve/ac:1f:6b:40:fc:e2`
4. Switch to iPXE's stock chain + autoinstall when ready (this GRUB path was purely to get
   into a live shell for inventory; the standard iPXE chain — snponly → boot.ipxe → mac
   file → menu.ipxe — is what should be used for the actual autoinstall once the VLAN port
   issue is fixed, since it's the one wired into the existing autoinstall-agent tooling).

## The machine

- **Host**: unimatrixone — Supermicro X10DSC+, dual LGA2011-v3 Xeon
- **PXE/data NIC**: Intel 82599 10GbE add-in card, MAC `ac:1f:6b:40:fc:e2` — this is the
  ONLY OS-usable data NIC (onboard NIC is dedicated to IPMI/BMC only, not a data path).
- **DHCP reservation**: `172.16.2.35` (updated 2026-07-02; was `172.16.3.203`/`.2.25`
  earlier in the day before the reservation was corrected).
- **IPMI**: `172.16.3.150` ADMIN/ADMIN — `ipmitool -I lanplus -H 172.16.3.150 -U ADMIN -P ADMIN ...`
- **CMOS battery**: replaced 2026-07-02 (was dead, firing repeated SEL `Battery Asserted`
  alerts). Replacing it reset the BIOS to Legacy mode — had to be switched back to UEFI.
- **Edge port / PortFast**: enabled on its switch port by the user (fixed initial
  `Link:down/Unknown` state seen at iPXE init; did not fix the VLAN issue).
- **Firmware**: BIOS 3.4 (2021-05-21, last release for this EOL board); BMC 4.00 (already
  ≥ latest published for the X10DSC family, ~3.88). No useful update exists.
- **82599 per-slot Option ROM**: found disabled in `Advanced → PCIe/PCI/PnP Configuration`
  on 2026-07-08 (a different setting from `Ipv6 PXE Support`) — likely blocks the card's PXE
  stack entirely regardless of IPv4/IPv6 config; reset again by the 2026-07-08 evening CMOS
  clear, needs re-enabling next session (see top section).
- **IPMI/IPMI-NIC theory (2026-07-08, unconfirmed)**: user suspects the actual PXE blocker may
  be in IPMI or the IPMI NIC configuration rather than host BIOS or the netboot server — not
  yet investigated, see "Next steps" in the top section.

## The netboot server (DO NOT power off)

- **Host**: unimatrixzero = `172.16.2.30` ("the server")
- nginx serves `/var/www/html/ipxe/` at `http://172.16.2.30/ipxe/`. Only ONE nginx site is
  enabled (`media.jdfalk.com.conf`) with a generic SPA catch-all at `location /` — new
  paths MUST go under an existing explicit `location` block (`/ipxe`, `/ubuntu`, `/isos`,
  `/cloud-init`) or they'll silently serve the SPA's `index.html` instead of a 404. (Learned
  the hard way: `/grub/grub.cfg` returned HTML until moved under `/ipxe/grub/`.)
- **tftpd-hpa** (NOT dnsmasq) serves TFTP: root `/srv/tftp`, listening `0.0.0.0:69`,
  `TFTP_OPTIONS="--secure --create"`. dnsmasq's own `enable-tftp` is intentionally commented
  out — this is NOT a sign TFTP is disabled.
- dnsmasq = proxy DHCP only (`/etc/dnsmasq.d/ubuntu-netboot.conf`), `log-dhcp` on. As of
  2026-07-07, `/etc/dnsmasq.d/ipv6-netboot.conf` also loaded (DHCPv6 pool + arch-16 match,
  see the 2026-07-07 section above).
- SSH as `jdfalk` works; `jdfalk` is in `adm` group so journals (dnsmasq, tftpd, nginx) are
  readable **without sudo**. `/var/www/html/ipxe/` is **writable without sudo** (ACL grants
  it). Interactive `sudo` over non-interactive SSH does NOT work (password prompt fails) —
  this is why the scoped sudoers helper above was created.
- `grub-mkstandalone`/`grub-mkimage` and a pre-built signed netboot GRUB
  (`/usr/lib/grub/x86_64-efi-signed/grubnetx64.efi.signed`) are already installed
  (`grub-efi-amd64-*` packages) — no need to build GRUB from source.
- **IPv6 netboot**: `radvd` (RA, `AdvManagedFlag on`, `AdvDefaultLifetime 0`) + dnsmasq
  DHCPv6 pool `fd37:59dd:a92e:a257::100`–`::1ff` on `enp8s0f0`, server itself at
  `fd37:59dd:a92e:a257::1`. Applied via `~/setup-ipv6-netboot.sh` (idempotent, re-run safe)
  on 2026-07-07 at 21:14. Confirmed active/enabled as of that timestamp.

## SOL access

```bash
# deactivate any stuck session first, then activate
ipmitool -I lanplus -H 172.16.3.150 -U ADMIN -P ADMIN sol deactivate
ipmitool -I lanplus -H 172.16.3.150 -U ADMIN -P ADMIN sol activate
```
Exit with `~.` (or `~~.` if you SSH'd in first, since the outer SSH eats one `~`).
Only one SOL session can be active at a time — reconnecting kicks any existing session.
To let Claude read a live session without taking it over, run interactively with:
```bash
ipmitool -I lanplus -H 172.16.3.150 -U ADMIN -P ADMIN sol activate 2>&1 | tee /tmp/uni-sol.log
```

## Boot sequence (confirmed via dnsmasq + nginx logs)

unimatrixone's UEFI firmware, on PXE, in UEFI mode:
1. Sends **arch 7** (`PXEClient:Arch:00007`) → dnsmasq offers `ipxe.efi` via TFTP. Firmware
   attempts it but reports `PXE-E21: Remote boot cancelled` and moves on (TFTP itself
   works fine when tested directly with curl — this is a firmware-side quirk, not a TFTP
   server problem).
2. Sends **arch 16** (`HTTPClient:Arch:00016`) → dnsmasq offers whatever URL is currently
   configured (`dhcp-boot=tag:httpboot,...` in `ubuntu-netboot.conf`, toggle via the
   sudoers helper). Firmware downloads it (`UefiHttpBoot/1.0` in nginx log) and executes it.

## Key files

- `/var/www/html/ipxe/grub/grubnetx64.efi` — current arch-16 target: standalone GRUB build
  with embedded auto-network-bringup config (WORKS through to DHCP attempt; blocked on the
  VLAN port issue past that point, on IPv4 — see 2026-07-07 section for the IPv6 path).
- `/var/www/html/ipxe/snponly.efi` — iPXE build (stock/unpatched driver + normal script),
  kept as the fallback path once VLAN is fixed and we want the original autoinstall chain.
- `/var/www/html/ipxe/boot.ipxe`, `/var/www/html/ipxe/boot/mac-ac1f6b40fce2.ipxe` — stock,
  restored, ready for the iPXE path.
- `/usr/local/sbin/unimatrixone-netboot-set.sh`,
  `/etc/sudoers.d/99-unimatrixone-netboot` — scoped passwordless sudo for switching the
  arch-16 target and reloading dnsmasq.
- `/tmp/grub-standalone.cfg`, `/tmp/grubnet_standalone.efi` (on server) — GRUB build
  source/output.
- `/tmp/ipxe/` (on server) — iPXE source clone, in case more iPXE-side work is needed later.
- `/etc/dnsmasq.d/ubuntu-netboot.conf` — proxy DHCP + arch matches (IPv4).
- `/etc/dnsmasq.d/ipv6-netboot.conf` — DHCPv6 pool + arch-16 match (IPv6, added 2026-07-07).
- `~/setup-ipv6-netboot.sh` (on server, in `jdfalk`'s home dir) — idempotent IPv6 netboot
  setup script (radvd + dnsmasq DHCPv6 config), applied 2026-07-07.
- `scripts/capture-uni-boot.sh` (this repo, run on the server) — `nohup`-safe combined
  tcpdump + dnsmasq + nginx capture wrapper for boot-attempt diagnostics, built 2026-07-08.
