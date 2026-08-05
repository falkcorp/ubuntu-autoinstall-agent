<!-- file: changelog.d/fix-tree-only-unlock-host-never-bound.md -->
<!-- version: 1.1.0 -->
<!-- guid: 343af6c3-2fd1-4b0e-8bc7-699cb83bc4c2 -->
<!-- last-edited: 2026-08-02 -->

### Fixed

#### A host whose Tang servers lived only in its `unlock_sss` tree installed clean and booted to an unsatisfiable LUKS prompt

`unlock_sss` (the recursive `SssPolicy` tree) and the nested clevis emitter
landed as two independent changes. Nothing connected them: every clevis decision
in the installer still keyed off the FLAT `tang_servers` roster, which a host
that declares its Tang servers inside the tree leaves empty. The result was a
host that installed no clevis packages, ran no `clevis luks bind`, baked an
initramfs with no network stack and no NIC driver — and **reported a successful
install**. It then booted to a LUKS passphrase prompt that no unattended factor
could answer, on hardware with no remote power. A bricked machine from a green
install.

Five guards now recognize an authored tree. `needs_clevis` and both
`enroll_tang_clevis` call sites (PlainLuks and NativeKeystore Phase 5) also
consider `unlock_sss.is_some()`. The `rd.neednet=1` / static `ip=` cmdline and
the forced-NIC-driver dracut fragment key off a new `uses_tang` predicate that
counts Tang anywhere in the tree — a bind the initramfs cannot reach the network
to satisfy is no better than no bind. `clevis-tpm2` is now installed whenever the
tree carries a tpm2 pin at any depth, not only on NativeKeystore: that pin's
decrypter ships in a separate package and its absence surfaces only in the
initramfs, at first boot, on an encrypted host.

The `clevis` dracut module is gated on the same predicate as the bind, not on
"uses Tang". Those were one expression before authored trees existed and so could
not diverge; now they can, and a Tang-less tree (tpm2-only, or the PKCS#11 OR
that `SssPolicy::any_pkcs11` builds) would otherwise be bound with clevis while
shipping an initramfs that cannot run it — the same bricking class in a new
shape.

The Tang advertisement pre-fetch walks `SssPolicy::tang_urls()` depth-first
instead of iterating the flat roster, so nested Tang entries get an `adv`. This
is load-bearing rather than cosmetic — without a pre-fetched advertisement
`clevis luks bind` prompts on `/dev/tty` and fails non-interactively over SSH.
The same server named twice in one tree is fetched once and referenced from both
pins.

### Added

#### Authored unlock trees emit their own nested clevis JSON

`build_clevis_policy_from_tree` is a pure recursive emitter alongside the
existing flat one, which is untouched and still serves every un-authored host
byte-for-byte (the len-serv-001/002 oracle and golden tests are unchanged and
still passing). A tree can nest to any depth and mix pin kinds, which the flat
signature could only represent by flattening — and a flattened `sss` group is
precisely the 2-of-4 weakening the tree type exists to make unrepresentable.

Each level emits `{"t":…,"pins":{…}}` with kinds grouped by
`SssPolicy::pins_by_kind`, and every kind emitted as an array, uniformly,
including a lone `tpm2`. An N-element array is N shares, so a 1-element array is
exactly one share: identical semantics to the bare object, with no per-kind
special case to get wrong as pins are added. A regression test proves a tree that
is nothing but a flat Tang group serializes identically to the legacy builder.

**The tree wins wholesale.** When one is authored, `config.tang_threshold` and
the forced NativeKeystore tpm2 peer share are both ignored — grafting either onto
an authored policy would silently change the share arithmetic its author
computed. A NativeKeystore tree with no tpm2 pin therefore gives up the default
peer share; that is honored as intent, and logged at `warn!` so it is visible in
the install log rather than discovered after a host fails to unlock.
