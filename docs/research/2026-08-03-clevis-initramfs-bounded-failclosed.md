<!-- file: docs/research/2026-08-03-clevis-initramfs-bounded-failclosed.md -->
<!-- version: 1.0.0 -->
<!-- guid: c41a9e2b-5d38-4f76-b0a1-93e7c5d6f284 -->
<!-- last-edited: 2026-08-03 -->

# Interactive PIN entry at root-unlock, and how to bound a clevis boot

**Measured 2026-08-03 in a purpose-built root-on-LUKS VM (`clevis-root`) on U0.
No physical hardware was touched.**

Two questions, both blocking the nested-`sss` rollout to the Raspberry Pi hosts
(172.16.2.45/.46/.47), which have **no remote power** — every unbounded boot
hang is a physical trip.

- **A.** Does interactive PKCS#11 PIN entry actually work for the **root**
  device, in the **initramfs**? Cold recovery depends on it.
- **B.** What actually bounds a clevis/systemd ask-password wait, and what
  happens when the bound expires?

## Test rig

Prior gate work used `/dev/vdb` as a **secondary** device via `/etc/crypttab`,
i.e. post-switch-root. Root unlock happens in the initramfs, a different code
path, so a new rig was built.

- `rootluks.qcow2` (12 G) attached to the existing `clevis-gate` builder VM,
  partitioned `vdc1` bios_boot / `vdc2` ext4 `/boot` / `vdc3` LUKS2 → `cryptroot`
  → ext4 `/`. The builder's running root was rsynced in, GRUB (i386-pc) and
  dracut installed from a chroot.
- Booted as its own libvirt domain `clevis-root` with
  `<serial type='pty'><log file=... append='on'/></serial>` for a durable
  transcript.
- Keyslot 0 = passphrase `gatepass` (recovery + console-liveness control),
  keyslot 1 = a **pkcs11-only** clevis binding against a SoftHSM token, bound
  deliberately **without** `pin-value=` so the interactive path is the only
  path.
- Ubuntu 26.04, systemd 259, dracut 110-11, clevis 23-1.

Console input reached the guest only through the emulated **keyboard**
(`virsh send-key`, i.e. tty0). Piping into `virsh console` never delivered a
byte, with either console ordering. Transcript capture and input were therefore
split: serial log for output, `send-key` for typing.

### Confounds caught before they produced a result

1. **`/etc/crypttab` keyfile.** With `cryptroot UUID=... none luks`, no PIN
   prompt ever appeared. `clevis-luks-pkcs11-askpin` iterates `/etc/crypttab`
   and acts **only** on lines matching `clevis-pkcs11.sock`:

   ```sh
   while read -r line; do
     if echo "${line}" | grep -E "clevis-pkcs11.sock" 1>/dev/null; then
   ```

   A `none` keyfile means the pkcs11 agent never sees the device. The entry must
   be `cryptroot UUID=... /run/systemd/clevis-pkcs11.sock luks`.

2. **A stale tracing wrapper.** `/usr/bin/clevis-decrypt-pkcs11` in the builder
   VM had been replaced by a concurrent agent with a 5-line wrapper
   (`sleep 3; exec /root/clevis-decrypt-pkcs11.upstream`) whose target is not in
   the initramfs. Every unlock failed with
   `line 5: /root/clevis-decrypt-pkcs11.upstream: No such file or directory`.
   Genuine upstream (sha256 `63ef4486…`, verified against the `clevis` .deb) was
   restored into the target so Part A measures stock clevis.

## Part A — interactive PIN entry at root-unlock: **YES**

Console transcript, run `A5-clean` (no `rd.debug`, verbatim except for ANSI
stripping and the systemd status-spinner lines being elided where marked):

```
[  OK  ] Found device dev-disk-by\x2duuid-8…f22691-1671-4579-bde2-11274ebb3bad.
         Starting systemd-cryptsetup@cryptr…Cryptography Setup for cryptroot...
[  OK  ] Finished systemd-networkd-wait-onl…ce - Wait for Network to be Online.
         Starting dracut-initqueue.service - dracut initqueue hook...
[    8.127794] dracut-initqueue[623]: Detected PKCS11 device:
Please, insert PIN for device with serial number:16cb4581ed58a6fb (UUID=80f22691-1671-4579-bde2-11274ebb3bad): (press TAB for no echo)
[*     ] (1 of 3) Job dracut-initqueue.service/start running (36s / no limit)
[**    ] (2 of 3) Job dev-disk-by\x2duuid-e0…ice/start running (37s / no limit)
[   40.811307] dracut-initqueue[623]: Device:/dev/disk/by-uuid/80f22691-1671-4579-bde2-11274ebb3bad unlocked successfully by clevis
[  *** ] (3 of 3) Job systemd-cryptsetup@cry…ice/start running (38s / no limit)
[  OK  ] Finished systemd-cryptsetup@cryptr…- Cryptography Setup for cryptroot.
[  OK  ] Found device dev-disk-by\x2duuid-e…3f94f0-1426-421c-88e5-2d9e674e641e.
[  OK  ] Reached target cryptsetup.target - Local Encrypted Volumes.
[  OK  ] Reached target initrd-root-device.target - Initrd Root Device.
[  OK  ] Reached target sysinit.target - System Initialization.
[  OK  ] Finished dracut-initqueue.service - dracut initqueue hook.
         Starting dracut-pre-mount.service - dracut pre-mount hook...
[  OK  ] Finished dracut-pre-mount.service - dracut pre-mount hook.
         Starting systemd-fsck-root.service…94f0-1426-421c-88e5-2d9e674e641e...
[  OK  ] Finished systemd-fsck-root.service…3f94f0-1426-421c-88e5-2d9e674e641e.
         Mounting sysroot.mount - /sysroot...
[  OK  ] Mounted sysroot.mount - /sysroot.
[  OK  ] Reached target initrd-root-fs.target - Initrd Root File System.
[  OK  ] Reached target initrd.target - Initrd Default Target.
         Starting dracut-pre-pivot.service …racut pre-pivot and cleanup hook...
[  OK  ] Finished dracut-pre-pivot.service - dracut pre-pivot and cleanup hook.
…
[  OK  ] Reached target multi-user.target - Multi-User System.
Ubuntu 26.04 LTS clevis-gate ttyS0
clevis-gate login:
```

The PIN typed was `111111`, which is **not** the LUKS passphrase, so the unlock
is unambiguously the clevis pkcs11 path.

**Negative control — the prompt is live and rejects.** Typing `999999` first:

```
Please, insert PIN for device with serial number:16cb4581ed58a6fb (UUID=…): (press TAB for no echo)
[  103.435967] dracut-initqueue[628]: Could not unlock device:/dev/disk/by-uuid/80f22691-…
ERROR:Unknown error. Please, insert PIN for device with serial number:16cb4581ed58a6fb (UUID=…): (press TAB for no echo)
```

**Console control.** Before the pkcs11 path was working at all, the plain LUKS
passphrase prompt reached this same console in the same boot and was answered
successfully from it. "No prompt" was therefore never confusable with "console
misrouted".

### `Ordering cycle ... clevis-luks-pkcs11-askpass.socket` does **not** occur here

Architectural, not incidental. `50clevis-pin-pkcs11/module-setup.sh` installs
**only dracut hooks** —

```sh
inst_hook pre-trigger 60      "${moddir}/clevis-pkcs11-prehook.sh"
inst_hook initqueue/settled 60 "${moddir}/clevis-pkcs11-hook.sh"
inst_hook initqueue/online 60  "${moddir}/clevis-pkcs11-hook.sh"
```

— and never installs `clevis-luks-pkcs11-askpass.socket`/`.service` into the
initrd. No unit in the initrd, so no ordering cycle is possible. Confirmed
empirically: the string never appears in any initramfs transcript, and the PIN
prompt is emitted by `dracut-initqueue[…]`, not by a systemd unit. The skip
message seen previously is a **post-switch-root** phenomenon and does not apply
to root unlock.

(By contrast, `50clevis/module-setup.sh` *does* install
`clevis-luks-askpass.path`/`.service` when systemd is in the initramfs, and the
transcript shows `Started clevis-luks-askpass.path` — that is the Tang/TPM2
path, and it is not the one that prompts.)

### Two upstream bugs found on the way

1. **`clevis_detect_pkcs11_device` cannot use a non-default PKCS#11 module in
   dracut mode.** In `/usr/bin/clevis-pkcs11-common` the fallback that reads
   `module-path=` out of the bound URI is gated behind
   `if [ "${dracut_mode}" != true ]` — so in the **initramfs**, the only place
   root unlock happens, it never runs. Detection is bare `pkcs11-tool -L`
   (OpenSC via pcscd). A token needing `libsofthsm2.so` or `libykcs11.so` can
   never be detected at root-unlock time; the boot instead loops on
   `Detected no PKCS#11 device, retry PKCS#11 detection? [yY/nN]`. A YubiKey
   driven through `opensc-pkcs11.so` is unaffected, because that *is* the
   default module. **Worked around in this rig** by making `libsofthsm2.so` the
   default module inside the image (gate hack only).

2. **`libpcsclite_real.so.1` is absent from the initramfs.** Ubuntu's
   `libpcsclite.so.1` is a dlopen shim; dracut's `50clevis-pin-pkcs11` installs
   only the shim. Measured console output:

   ```
   dracut-initqueue[5603]: loading "libpcsclite_real.so.1" failed: libpcsclite_real.so.1: cannot open shared object file: No such file or directory
   dracut-initqueue[5603]: No slots.
   ```

   pcscd could enumerate no readers. **Not verified against real hardware** —
   SoftHSM bypasses pcsc-lite entirely, so this rig cannot exercise the YubiKey
   path. It is a strong signal, not a proof. `verify-initramfs.sh` now checks
   for it.

## Part B — bounding the failure

### The 902 s hang was a *detector* failure, not an *action* failure

Nothing had failed. systemd was contentedly waiting on a password. The console
says so:

```
(1 of 3) Job dracut-initqueue.service/start running (36s / no limit)
```

and on the guest:

```
$ systemctl show dracut-initqueue.service -p TimeoutStartUSec -p JobTimeoutUSec
TimeoutStartUSec=infinity
JobTimeoutUSec=infinity
```

The clevis PIN prompt runs **inside `dracut-initqueue.service`**, not inside
`systemd-cryptsetup@cryptroot.service`. That is why `x-systemd.device-timeout=`,
`rd.timeout=`, `crypttab timeout=` and `rd.luks.options=timeout=` bounded
nothing: they govern the wrong unit. In run `A5-clean`,
`systemd-cryptsetup@cryptroot` completed in ~2 s once the passphrase arrived
over the socket.

### What does NOT bound it (all measured)

| Candidate | Result |
|---|---|
| `x-systemd.device-timeout=90`, `rd.timeout=120` | no effect (prior 902 s measurement); governs device appearance / initqueue, not an ask-password wait |
| `dracut-initqueue.service` defaults | `TimeoutStartUSec=infinity` |
| `systemd.crash_reboot` | fires only on PID 1 crash; never entered |
| dracut's own initqueue timeout | run `B4`: with nobody typing, `clevis-luks-pkcs11-askpin` blocks in the `initqueue/settled` hook, so the initqueue loop never reaches its timeout at all. Fired in run `B3` only *after* clevis gave up |
| clevis `too_many_errors=3` | run `B3`: prints `Too many errors !!!` at 155.9 s, then the boot **keeps hanging** on the unbounded plain-passphrase fallback — still running at 4 min 15 s. Does not terminate anything |

### What DOES bound it

A drop-in on **`initrd.target`'s job**. That job stays queued until every initrd
unit finishes, so one knob covers a hang anywhere in the initramfs — the PIN
prompt, the passphrase fallback, or a device that never appears.

```ini
# /usr/lib/systemd/system/initrd.target.d/10-uaa-unlock-deadline.conf
[Unit]
JobTimeoutSec=600
JobTimeoutAction=reboot-force
```

Run `B1` (deadline set to 120 s for a fast experiment, nothing typed):

```
[  !!  ] Forcibly rebooting: job timed out, proceeding in 5s
[  129.794147] reboot: Restarting system
```

and the machine came back up and re-armed the PIN prompt — an unattended retry
loop, which is the only thing that helps a host whose Tang servers may simply
have come back.

Run `B2` (same 120 s deadline, correct PIN typed at ~35 s): unlocked at 36.6 s,
single boot, reached `login:`. **The deadline does not fire on a legitimate
unlock.**

### The companion that is NOT optional: `rd.shell=0 rd.emergency=reboot`

Run `B3` set the deadline to 600 s and left `rd.shell` at its default. dracut's
emergency path won the race and the boot ended at

```
Press Enter for system maintenance
(or press Control-D to continue):
```

`emergency.target` cancels the `initrd.target` job, so `JobTimeoutAction` never
fired. An emergency shell on a host with no remote power is exactly the outcome
we are trying to eliminate.

Run `B4`, identical but with `rd.shell=0 rd.emergency=reboot` on the cmdline and
a 900 s deadline:

```
[  !!  ] Forcibly rebooting: job timed out, proceeding in 5s
[  909.742077] reboot: Restarting system
```

Two boots in the transcript, `insert PIN` twice — reboot, retry, prompt again.

Run `B5` re-ran the same proof driven by this repo's dracut module
(`dracut/92uaa-unlock-deadline`, `uaa_unlock_deadline_sec=120`) rather than a
hand-placed file: forced reboot at 129.1 s, 2 boots, 2 prompts.

### Fail-closed

`reboot-force` is a hard reset. There is no code path from the timeout to a
mounted root, and none to a degraded or partially-unlocked boot: the real root
is not mounted when it fires, so there is nothing to fall through to. The
recovery path after the reset is the same one as before it.

## Recommended configuration

Kernel cmdline (`GRUB_CMDLINE_LINUX`), in addition to the existing
`rd.neednet=1 ip=dhcp rd.luks.uuid=…`:

```
rd.shell=0 rd.emergency=reboot
```

`/etc/crypttab` for any device carrying a clevis **pkcs11** binding — the socket
keyfile is mandatory, not cosmetic:

```
cryptroot UUID=<luks-uuid> /run/systemd/clevis-pkcs11.sock luks
```

`/etc/dracut.conf.d/`:

```
add_dracutmodules+=" uaa-unlock-deadline "
uaa_unlock_deadline_sec=600
```

### Why 600 s

- The measured successful unlock is ~30 s (non-interactive Tang path); in this
  rig the PIN prompt appeared at 8.1 s and the unlock completed 1.5 s after the
  PIN was submitted. 600 s is 20× that.
- It must be an **outer** bound that exceeds the inner budget of the
  concurrently-developed `clevis-decrypt-pkcs11` fork
  (`docs/research/2026-08-03-clevis-pkcs11-multitoken-pin-fork.md`), which sets
  `systemd-ask-password --timeout=120`, `UAA_MAX_TRIES=2` and `flock -w 300`.
  Worst-case legitimate human time under those constants is ~240 s of prompts,
  and a concurrent-`sss` share can wait up to 300 s on the lock. A 300 s outer
  bound would leave no margin and would fire before their own retry logic
  finished; 600 s clears it. **These two numbers must be changed together.**
- With RPi POST + GRUB (~30–45 s), 600 s yields an unattended Tang-outage retry
  roughly every 10–11 minutes. Fast enough that a Tang server coming back is
  picked up within one cycle; slow enough not to be a reboot storm.

### "Is a human typing?" — honest answer: partly, and we do not use it

`JobTimeoutSec` is a fixed wall clock from the `initrd.target` job start. **It
cannot be reset.** It will fire on an operator who walks up at minute 9 and
types slowly. The consequence is a reboot and a fresh prompt about a minute
later, not a lockout, which is why the value is sized generously instead of made
adaptive.

A reset-capable variant is possible: a watchdog could stat the newest
`/run/systemd/ask-password/ask.*` (a new file means a query was answered and
re-armed) or the fork's `${PIN_FILE}.${UAA_ID}.tries` file, and restart its
timer on each **submitted attempt**. That detects attempts only — it still
cannot distinguish a person mid-keystroke on their *first* attempt from an empty
room, which is the case that actually matters. It was rejected here because a
bespoke watchdog process that survives `switch-root` could reboot a healthy
machine, and `JobTimeoutSec` is systemd-native and structurally cannot leak past
the initramfs. Do not read the console tty to detect keystrokes: the password
prompt owns it in raw mode and a second reader steals input.

## Not determined

- **Real YubiKey through pcscd.** SoftHSM bypasses pcsc-lite, so the
  `libpcsclite_real.so.1` gap and the whole CCID path are unexercised. Needs a
  hardware token in a passthrough VM.
- **`initramfs-tools`.** Everything here is dracut, which is what this repo
  emits (`InitramfsType::Dracut` is the `base_image` default). The unexecuted
  `TASK-23` brief specifies `initramfs-tools` for the rpi-serv group and
  `examples/configs/install/rpi-serv-001.yaml` does not exist yet. Under
  `initramfs-tools` there is no systemd in the initrd, no `initrd.target`, and
  therefore **none of the Part B mechanism applies** — `clevis-initramfs` is a
  different code path entirely and is not even installed on the gate image.
- **What .45/.46/.47 actually run today.** Out of scope by instruction (no
  physical hosts touched), and it decides which of the two paths above governs.
  Determine this before applying anything here to those hosts.
- **arm64.** Everything measured is x86-64/SeaBIOS. GRUB and dracut behaviour on
  the RPi boot path is unverified.
- **Interaction with the nested-`sss` policy.** The rig used a pkcs11-**only**
  binding to isolate the interactive path. A real `sss` policy forks its shares
  concurrently; how the outer deadline composes with several simultaneous PIN
  prompts is covered by the fork's `flock`, but was not boot-proven here.
