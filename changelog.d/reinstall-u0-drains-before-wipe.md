<!-- file: changelog.d/reinstall-u0-drains-before-wipe.md -->
<!-- version: 1.0.0 -->
<!-- guid: 8c51a06d-3f92-47be-b8d4-1e7a35c96028 -->
<!-- last-edited: 2026-07-30 -->

### Added

#### U0 drains cluster membership before a reinstall wipes the disk

`reinstall_machine` gains a `NodeDrainer` seam. Before power-cycling a host
into an installer, U0 decommissions any clustered workload it runs and blocks
until the replica count reaches zero. Hosts with no clustered workload skip the
step entirely (`needs_drain` returns false), so a Tang server or a bare
install-target pays nothing.

**U0 owns this, not the host being reinstalled.** A node cannot reliably watch
its own decommission finish: the drain needs it up and serving while something
polls to completion, and that something is about to be power-cycled. U0 already
holds the CA and issues the node certs, so it is the only party that can both
drive the decommission and observe it complete. It also avoids putting a
cluster-admin credential on every machine.

**Ordering is load-bearing.** Decommission is terminal — a CockroachDB node can
never rejoin under the same node id. So the drain runs *after* the reversible
work (registry write + dual-layer boot-target projection, both undoable) and
*before* the power cycle (the first destructive act). Draining any earlier
would mean a projection failure left a healthy node permanently decommissioned
for nothing, which is precisely the state len-serv-003 has been sitting in:
decommissioned, still running, unable to rejoin.

Both drain failure modes are **fail-closed**, adding
`RefusalReason::DrainIncomplete { replicas_left }` and
`RefusalReason::DrainFailed(String)`. A partial drain or an unreachable node is
a refusal, not a warning: power is never invoked and both layers are flipped
back to `local-disk`, because wiping a node that still holds replicas is the
one outcome this step exists to prevent. Tests assert that power is never
invoked on either path.
