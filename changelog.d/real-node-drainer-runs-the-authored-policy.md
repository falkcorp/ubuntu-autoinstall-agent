<!-- file: changelog.d/real-node-drainer-runs-the-authored-policy.md -->
<!-- version: 1.0.0 -->
<!-- guid: 9d41c7a2-5e86-4b03-8f19-2c6e0b5d7a34 -->
<!-- last-edited: 2026-07-30 -->

### Added

#### U0 executes the authored drain policy before a reinstall wipes a host

`uaa-control` gains `drain.rs`: `SpecDrainer`, the real `NodeDrainer` behind
`reinstall`'s seam. `needs_drain` is `requires_drain()` over the host's resolved
applications; `drain` executes the `DecommissionStep`s each enabled policy
declares, honouring its own `timeout_secs` / `poll_interval_secs`.

Nothing in the module is hostname-aware — no `if len-serv-*`, no list of
database hosts. A host earns a safe reinstall by authoring the block on its
spec.

Everything is argv, never a shell string. Unit names and addresses come out of
the registry, and there is no interpolation step for a quoting mistake to hide
in. `parse_node_status` locates columns **by header name**, never by position:
CockroachDB has reordered columns across versions and the fleet is mid-flight
between v25.3 and v25.4, so a positional parse could read some other column as
`replicas` and report a false zero.

Seams (`DrainTargetResolver`, `CommandRunner`, `Clock`) keep the whole thing
testable without a cluster; 14 tests cover it, including a virtual clock that
drives the timeout path to completion.

### Fixed

#### The default cockroach drain order was backwards

`DecommissionPolicy::cockroach_default()` was stop-unit → decommission →
wait-for-zero-replicas. CockroachDB moves replicas off a node *while that node
is still running and serving* — `cockroach node decommission` asks a live node
to hand its ranges away. Stopping `cockroach.service` first would have left the
ranges to re-replicate only via the dead-node timeout, which is precisely the
under-replication window the policy exists to close.

Now decommission → wait-for-zero-replicas → stop-unit, with the unit stopped
last, once the node is provably holding nothing. A test asserts the stop is the
final call, so the order cannot silently regress.

`cockroach node decommission` is invoked with `--wait=none` so the authored
`timeout_secs` owns the deadline rather than `cockroach`'s own internal wait.

### Notes

Every ambiguity fails closed, toward *refuse the reinstall*: an unparseable
`node status`, a missing `id`/`address` column, one IP matching two node ids, a
roster row whose replica count cannot be read, or a count that will not reach
zero inside the deadline. An incomplete drain returns
`DrainStatus::Incomplete` and later steps do not run — the unit is never
stopped and `reinstall` never reaches the power cycle.

The single deliberate exception: a host the cluster has **never heard of**, or
has already fully decommissioned, is `Drained`. It holds no replicas by
definition, so there is nothing a drain could accomplish. This is len-serv-003's
actual state — decommissioned long ago, still running, unable to rejoin — and
erroring there would make an already-safe host permanently un-reinstallable.
