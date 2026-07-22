### Fixed

#### ARP discovery scanner crashed on unresolvable IPs

The identity-resolving scanner ran `getent hosts <ip>` inside a `$(...)` under
`set -euo pipefail`; `getent` exits 2 on a not-found address (phones, IPv6
link-local, unnamed hosts), which killed the whole scanner (`status=2`) on the
first unresolvable device. Guarded the `getent` and `grep` command substitutions
with `|| true`. Runtime-verified on the server.
