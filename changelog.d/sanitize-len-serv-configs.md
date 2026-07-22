<!-- file: changelog.d/sanitize-len-serv-configs.md -->
<!-- version: 1.0.0 -->
<!-- guid: 7b1e9c34-2a56-4d80-9f21-3c8a6e0d5b42 -->
<!-- last-edited: 2026-07-22 -->

### Security

#### Strip committed MAC addresses from the len-serv configs

The `len-serv-*.yaml` "Fleet fact" comments committed each host's real MAC
address (a spoofable identifier) plus IP/NIC to git. Removed them — host-unique
facts belong in the registry backend, not the repo. Comments only; no schema or
behavior change. (Follow-up: move the real per-host values fully to the registry
and key host trust on the enrollment SPKI/TPM-EK, not the MAC.)
