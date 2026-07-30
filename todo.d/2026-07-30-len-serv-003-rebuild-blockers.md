- [ ] **dash-003** Provision AMD DASH on len-serv-003 (172.16.3.96) so the
      rebuild is remotely recoverable. Ports 623/664 are closed there while
      len-serv-001/002 are live; BIOS `DASH Support` already reads `Enabled` via
      ThinkLMI with no BIOS admin password, so the firmware listener was never
      started. Try in order: toggle the ThinkLMI attr + reboot, full AC
      power-cycle (a warm reboot is reportedly insufficient), then `DASHConfigRT`.
      Until this works, a netboot that misses PXE means a physical trip.
- [ ] **dash-auth** Solve authenticated wsman against DASH. `wsman identify -P 623`
      works unauthenticated on len-serv-002, but authenticated enumerates return
      empty with Administrator/Password under both digest and basic. No scripted
      power control until this is answered.
- [ ] **seed-trap** Retire or clearly mark the old PlainLuks/LVM PXE seed for
      len-serv-003 on the server: `/var/www/html/cloud-init/6c4b90bcf7f4/user-data`
      plus `ipxe/boot/mac-6c4b90bcf7f4.ipxe`. Flipping that menu-default to
      `autoinstall` today rebuilds the exact layout the native-keystore work
      replaces. See `docs/len-serv-003-preflight-inventory-2026-07-30.md`.
- [ ] **netboot-doc** Correct `docs/netboot-autodeploy.md`. It calls
      len-serv-003's `user-data` "the known-good template — do not fix it, it
      works" and says 001/002 were regenerated from it. The live machines show
      the opposite: 001/002 root on ZFS (`rpool/ROOT/...`), 003 on ext4 over
      LVM over LUKS. It also documents `/api/health` and a
      `target=custom-autoinstall` flip value; the deployed service returns
      `{"error":"not found"}` for `/api/health` and the real iPXE menu label is
      `autoinstall`.
- [ ] **app-specs** Model the remaining len-serv applications as `ApplicationSpec`
      variants so a rebuilt host returns fully configured — currently only
      `Cockroach` (and `TangServer` from PS-APP-09) exist. Needed:
      prometheus-node-exporter, landscape-client, canonical-livepatch,
      cockroach-rollout-agent, `report-status.sh`, zsh/oh-my-zsh, and DASH
      provisioning. Inventory and per-application requirements are in
      `docs/len-serv-003-preflight-inventory-2026-07-30.md`.
- [ ] **crdb-flags** Standardize the Cockroach application on the len-serv-001/002
      flag form. len-serv-003 drifted to `--listen-addr`/`--sql-addr` bound to its
      own IP (so `127.0.0.1:36257` is refused) and `--store=/var/lib/cockroach/data`
      without `attrs=ssd,size=.5`. Whatever the redeploy emits should match
      001/002, not 003.
