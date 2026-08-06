<!-- file: changelog.d/live-boot-carries-a-diagnostic-seed.md -->
<!-- version: 1.0.0 -->
<!-- guid: 9a4c7e21-3f86-4d19-b0c5-8e72a1f6d340 -->
<!-- last-edited: 2026-08-05 -->

### Fixed

- **A netbooted live host now comes up with `sshd` and the operator's keys, so
  `uaa ssh-install` can actually reach it.** The iPXE `:live-amd64` and
  `:live-arm64` entries carried no `ds=`, so the live session had no cloud-init
  datasource, no user, and no ssh server. Booting a host into "live" and then
  installing to it over SSH — the whole remote-rebuild path for a machine with
  no remote power — could not work, and the failure only showed up at the point
  where someone was standing in front of the rack.

  The live entries now seed from `http://172.16.2.30/cloud-init/live/`, which is
  a **separate** seed from the per-MAC autoinstall directories. That separation
  is the point, not an implementation detail: the per-MAC seed contains an
  `autoinstall:` document, so pointing the live entries at it — the obvious
  one-line fix, since those keys are already there — would have turned
  "boot live to look at the machine" into an unattended repartition, using the
  old PlainLuks/LVM layout that is still seeded for len-serv-003 and is no
  longer the layout the installer builds.

### Added

- **`netboot/ipxe/menu.ipxe` and `netboot/cloud-init/live/` are now in the
  repository.** `menu.ipxe` previously existed only at
  `/var/www/html/ipxe/menu.ipxe` on U0, so every change to the fleet's boot menu
  was unreviewable, unattributable, and lost on any rebuild of that tree. The
  repo copy is the source of truth; the server copy is deployed from it.
