<!-- file: docs/agent-tasks/DEFERRED.md -->
<!-- guid: 3954f24c-5488-4bff-bf6a-3ea3351f7645 -->
<!-- version: 1.0.0 -->
<!-- last-edited: 2026-07-09 -->

# Deferred — needs hardware/server investigation; deliberately NOT tasked

These items are real, but each requires physical access, live-host investigation, or
credentials that do not exist yet. Writing an agent brief for them now would produce
guesswork. Revisit each after its blocker clears; none blocks the 20 planned tasks.

| Item | Why deferred | Unblocks when |
|---|---|---|
| **SUM read-only OpROM-mode preflight** (abort md/IMSM installs when the BIOS SATA controller mode would make the array unbootable — the exact failure U1 hit) | The specific BIOS token name/value must be read from U1's own `GetCurrentBiosCfg` dump; guessing token names produces a preflight that green-lights the wrong mode | An operator runs SUM in-band on U1 and records the storage-OpROM token (todo.md "SUM read-only check") |
| **U1 PXE / first-boot-didn't-boot debugging** (with `ubuntu` #1 + BootNext set, U1 still booted the USB live env) | Requires watching IPMI SOL during a real boot to see where the chain dies (shim/grub vs initramfs md-assembly vs Tang unlock); IPMI must run from the server | An operator captures SOL (`ssh 172.16.2.30 ... sol activate`) across one cold boot |
| **AMD DASH / Intel AMT driver + credential setup on len-serv-001/002/003** | Realtek DASH driver + DASHConfigRT credentials are not installed on any lenserv; needs on-host installs + reboots + physical fallback if it bricks networking | Driver installed + credentials configured on at least one M715q (todo.md "Lenovo M715q — AMD DASH") — then implement the `AmdDash` arm stubbed in remote-power/TASK-01 |
| **unimatrixone / new-machine registration + disk-topology discovery flow** | The registration steps (todo.md "New Machines") need a booted host to interrogate (`lsblk`, `/proc/mdstat`) — U1 is mid-bring-up and the flow should be designed against a real second machine | Next new machine arrives, or U1 finishes bring-up and the flow can be replayed against it |
| **CockroachDB decommission / rejoin for len-serv-003 reimage** | Cluster surgery on live prod data: needs the RF=3 decommission sequence (node 8 + ghost node 3 cleanup), root client certs on the server, and a healthy cluster window; also gated on the QEMU VM validation (testing-gates/TASK-01) passing first | VM gate green AND an operator schedules the decommission window |

Hard rule reminder: none of these may be attempted by an autonomous task — every one
touches live infrastructure (the server, prod CockroachDB, or physical BIOS/BMC).
