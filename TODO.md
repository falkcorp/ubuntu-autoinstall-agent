# Ubuntu Netboot/Autoinstall TODO

## Immediate Priority: Validation and Stability

- [ ] Run Subiquity autoinstall validation against active served configs (`/var/www/html/cloud-init/.../user-data`)
- [ ] Verify end-to-end netboot install on `len-serv-003` with Ubuntu 26.04
- [ ] Confirm installer logs and first boot logs are captured for regression triage
- [ ] Freeze a known-good baseline config before adding advanced unlock features

## FDE + TPM + Tang + YubiKey Plan (Post-Validation)

### Goals

- Auto-unlock at boot when network is available and Tang is reachable
- No weak static passphrase as primary unlock method
- Strong manual fallback when auto-unlock fails
- Consistent behavior across:
  - `len-serv-001` to `len-serv-003`
  - `rpi-serv-001` to `rpi-serv-003`

### Design Work

- [ ] Define threat model and unlock policy:
  - Tang-only auto unlock conditions
  - TPM2 binding requirements
  - Manual fallback requirements (YubiKey + PIN or equivalent)
- [ ] Choose target boot/initramfs stack assumptions per host class (dracut-based path)
- [ ] Decide supported fallback combinations:
  - TPM2 + Tang
  - TPM2 + YubiKey challenge-response or PIV/FIDO-backed workflow
  - Recovery key escrow/backup policy

### Autoinstall Integration

- [ ] Add dracut-based crypt packages in autoinstall profiles (baseline done for `len-serv-003`; generalize to all host profiles)
- [ ] Add post-install automation to:
  - Enroll LUKS slot(s) for Clevis Tang
  - Enroll TPM2-backed unlock path
  - Configure dracut modules and regenerate initramfs
  - Validate unlock bindings before reboot
- [ ] Add machine-specific Tang endpoint and trust data templating
- [ ] Add optional YubiKey enrollment step for each machine

### YubiKey Integration

- [ ] Standardize YubiKey mode to use (HMAC challenge-response vs PIV/FIDO-assisted flow)
- [ ] Automate per-host enrollment procedure with auditable output
- [ ] Ensure fallback prompts are deterministic on boot failure paths
- [ ] Document operational runbook for replacing a lost YubiKey

### Recovery and Backup

- [ ] Add secure recovery key generation and storage workflow
- [ ] Add documented break-glass procedure when:
  - Tang unavailable
  - TPM state changed (firmware/motherboard events)
  - YubiKey unavailable
- [ ] Add quarterly recovery test checklist

### Validation/Test Matrix

- [ ] Online boot with Tang reachable (expect auto-unlock, no prompt)
- [ ] Offline boot with Tang unreachable (expect manual secure fallback)
- [ ] TPM mismatch simulation (expect fallback path works)
- [ ] YubiKey missing/wrong PIN behavior
- [ ] Reboot loop and unattended restart behavior checks

## Implementation Notes

- Do not enable insecure root/password SSH defaults as part of unlock automation.
- Keep `ds=nocloud` datasource usage for this environment.
- Keep changes incremental: one host (`len-serv-003`) first, then roll out.
