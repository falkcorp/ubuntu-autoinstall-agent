<!-- file: changelog.d/decommission-declared-by-application-spec.md -->
<!-- version: 1.0.0 -->
<!-- guid: 4b96e7c1-2d38-4f50-a7e6-91c0b34d8f25 -->
<!-- last-edited: 2026-07-30 -->

### Added

#### Drain policy is declared by the application, not hardcoded in U0

`CockroachSpec` gains a `decommission` block, and `ApplicationSpec` gains a
`decommission()` accessor plus the free functions `requires_drain()` and
`drain_steps()`.

```yaml
- kind: cockroach
  seed_ip: 172.16.3.92
  decommission:
    enabled: true
    steps:
      - step: stop-unit
        unit: cockroach.service
      - step: cockroach-decommission
      - step: wait-for-zero-replicas
    timeout-secs: 3600
    poll-interval-secs: 30
```

This makes `NodeDrainer::needs_drain` ask *"does any application on this host
declare `decommission.enabled`?"* instead of *"is this host one I remember runs
a database?"*. A future clustered workload becomes safe to reinstall by
authoring the block on its variant — the reinstall driver never changes.
Stateless applications return `None` and cost nothing.

`DecommissionStep` is a **closed enum, deliberately not free-form shell.**
These steps run on **U0**, the fleet control plane — unlike `HookStep`, which
runs on the target being installed and whose blast radius is a machine already
headed for a wipe. Letting a registry profile blob execute arbitrary commands
on U0 is a far larger promise, and closed-but-growing enums are how every other
component here is modelled (Decision 15).

The cockroach default is `enabled: true` with stop-unit → decommission →
wait-for-zero-replicas. The `decommission` key is deliberately **not**
`skip_serializing_if`: a policy governing a terminal, destructive operation
should be visible in the placed artifact rather than implied by its absence.

### Fixed

`validate_resolved` gains rule 6: an application setting `decommission.enabled`
while declaring no steps is now a validation error. That combination is the
worst possible state — `needs_drain` says yes, the drain is a no-op, and the
reinstall proceeds to wipe a node still holding replicas. Caught at authoring
time instead of during a rebuild.
