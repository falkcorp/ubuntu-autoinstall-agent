<!-- file: todo.d/2026-08-04-rpi-serv-is-tang-only.md -->
<!-- version: 1.0.1 -->
<!-- guid: 8e14d7b2-3a95-4c68-b1f0-6d2c9a45e731 -->
<!-- last-edited: 2026-08-04 -->

- [ ] **rpi-no-crdb** When TASK-23 (PS-MIG-RPI-24) authors the `rpi-serv` group,
      give it `applications: [tang-server]` and nothing else. CockroachDB is
      **off** the RPis as of 2026-08-04: the only live nodes are 4/5/6/9
      (172.16.2.30, 172.16.3.94, 172.16.3.92, 172.16.2.35) with ~220 replicas
      each and `ranges_under_3_replicas = 0`. No RPi address appears among them.
      Five ids (1, 2, 3, 7, 8) read `membership=decommissioned`; which id was
      which host is *not* recoverable — decommissioned rows carry `address=NULL`
      — so don't cite the mapping. Purge scripts live on U0 at
      `/home/jdfalk/ai/cockroach-rpi-decom/`. The repo has no rpi-serv config
      today, so this is a "do not add it back" note rather than a removal.
- [ ] **u0-join-list** U0's `cockroachdb.service --join` names only
      172.16.2.45/.46/.47 — the three RPis about to be wiped. Harmless while the
      process is up (gossip already found the real peers) but a restart after the
      purge would have zero reachable join targets. Fix staged at
      `/home/jdfalk/ai/cockroach-rpi-decom/10-fix-u0-join.sh` (needs the operator's
      sudo). If the installer ever manages U0's Cockroach unit, the join list must
      be derived from live membership, not from a static seed list.
- [ ] **rpi-dracut** The `rpi-serv` group must set `initramfs: dracut`. The brief
      originally said `initramfs-tools`; that is now rejected outright by
      `validate.rs` rule 7 and by `InstallationConfig::from_yaml_file`, so the
      brief as written would have failed its own acceptance criteria. Fixed in
      this change, but re-check it when the brief is actually executed.
