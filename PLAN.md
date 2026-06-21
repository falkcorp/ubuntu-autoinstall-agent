# Pivot: from imperative ZFS installer → autoinstall user-data renderer

## Goal
Pivot the tool away from reimplementing an installer (debootstrap + ZFS + chroot over
SSH) toward **generating the proven, known-good subiquity autoinstall `user-data`** (the
hand-tuned len-serv-003 config) parameterized per host, then driving the native Ubuntu
installer and validating the result. This first PR delivers only **slice one**: a pure,
unit-tested renderer that reproduces the known-good artifacts byte-for-byte. No server
placement, no flip, no reboot yet.

## Why this shape (decisions already made, not open questions)
- **Template as text, not structure.** The payload is a large inline bash heredoc; modeling
  it structurally would be self-harm. We template the committed 003 `user-data` as text.
- **Emit directly; REPLACE `register-gen.py`.** Confirmed by reading it on the server:
  it is *stale* — it still emits the old broken late-commands (external chroot script +
  manual `mount --bind`/`chroot`) and its chroot script has no clevis bind, no dracut
  network-unlock config, no `curtin in-target`. The known-good 003 user-data is ahead of
  it. This tool becomes the source of truth for the template; **once the generator works
  we retire register-gen.py entirely** (it is not a backport target).
- **Template is overridable, not hard-coded.** The known-good 003 template ships embedded
  as the default (`include_str!`), but a `--template <file>` flag lets anyone supply their
  own template file. Placeholders are documented so future changes are a template edit, not
  a Rust change. This keeps the tool a thin, data-driven renderer.
- **The substitution set is already proven.** This session's verified 001/002-vs-003 diff
  *is* the template spec: `hostname` (identity + messages + flip path + variables HOSTNAME),
  `network_address` (NET_ET_ADDRESS + COCKROACH_ADVERTISE), `cockroach_join`. Nothing else.
- **Leave the ZFS installer code untouched.** Manual install is on hold per user. The new
  renderer is additive; rip out nothing.

## Affected files (slice one)
- `tests/fixtures/golden/len-serv-001.user-data` — NEW. Exact copy pulled from the server.
- `tests/fixtures/golden/len-serv-002.user-data` — NEW. Exact copy pulled from the server.
- `tests/fixtures/golden/len-serv-003.user-data` — NEW. Exact copy (the template source).
- `src/autoinstall/mod.rs` — NEW. Module export.
- `src/autoinstall/host_spec.rs` — NEW. `HostSpec` struct (the per-host inputs) + a
  `cockroach_join` helper (server + the two other lenservs, excluding self) and
  `from_installation_config` adapter so existing config/YAML can feed it.
- `src/autoinstall/render.rs` — NEW. The renderer: `render_user_data(template: &str,
  spec: &HostSpec) -> Result<String>`, plus `default_template() -> &'static str`
  (`include_str!` the embedded 003 template) and a loader that uses `--template <file>`
  when provided, else the default. Substitution is an explicit map of documented
  placeholders; an unknown/leftover `{{...}}` placeholder is a hard error (so a bad
  custom template fails loudly rather than shipping a literal `{{FOO}}`).
- `src/autoinstall/templates/len-serv.user-data.tmpl` — NEW (embedded default). The 003
  user-data with the proven substitution points turned into placeholders (e.g.
  `{{HOSTNAME}}`, `{{NET_ADDRESS}}`, `{{COCKROACH_ADVERTISE}}`, `{{COCKROACH_JOIN}}`).
  Everything else byte-identical to 003. Header comment documents every placeholder so
  others can fork it.
- `src/lib.rs` — add `pub mod autoinstall;`.
- `src/cli/args.rs` — add a `RenderUserData` subcommand (`--config <yaml>` or explicit
  flags; `--template <file>` optional override; `--output <file>` default stdout). Parsing
  test alongside existing ones.
- `src/cli/commands.rs` — `render_user_data_command()` handler.
- `src/main.rs` — dispatch arm.

## Steps (each independently reviewable / committable)
1. **Fixtures.** Pull the three known-good `user-data` files verbatim from the server into
   `tests/fixtures/golden/`. Commit as-is (these are the ground truth the test asserts on).
2. **Template.** Create the embedded default `len-serv.user-data.tmpl` by copying the 003
   fixture and replacing ONLY the proven substitution points with placeholders. Diff
   template-with-003-values against the 003 fixture to prove zero incidental changes.
   Document every placeholder in a header comment.
3. **Renderer + HostSpec.** Implement `HostSpec`, `cockroach_join`, `render_user_data`
   (takes a template string), `default_template()`, and the `--template` loader. Pure
   functions, no I/O except the optional template-file read. Leftover `{{...}}` = error.
4. **Golden tests.** Unit tests that render the **default template** with 001/002/003 params
   and assert **byte-equality** against each fixture. Plus a test that a custom template
   with an unfilled placeholder errors. This is the whole point — impossible to regress
   silently against the known-good.
5. **CLI wiring.** `render-user-data` subcommand (incl. `--template`) → renderer →
   stdout/file. Add a parse test.
6. **Docs.** Short section in `docs/netboot-autodeploy.md`: the renderer is the template
   source of truth, how to supply a custom `--template`, the placeholder list, and that
   register-gen.py is slated for retirement once placement/drive lands.

## Test strategy
- Commands: `cargo test` (must stay green; currently 156 pass) and specifically
  `cargo test --lib autoinstall`.
- Success criteria:
  - `render_user_data` with 003's params == `tests/fixtures/golden/len-serv-003.user-data`
    byte-for-byte.
  - Same for 001 and 002 params against their fixtures.
  - `cargo build` clean; clippy no new warnings.
  - `cargo run -- render-user-data --config examples/configs/...yaml` prints a valid
    user-data (spot-check it `#cloud-config`-parses).

## Explicitly OUT of scope (later slices, noted so they're not forgotten)
- **Slice two — `verify <host>`:** SSH into an installed host and validate it matches intent
  (crypto_LUKS present, `clevis luks list` shows SSS/tang, dracut cmdline has
  `rd.neednet=1 ip=dhcp`, crypttab, services up). This is where the tool clearly beats
  register-gen.py.
- **Slice three — placement & drive:** write the seed into the server netboot tree
  (`/var/www/html/cloud-init/<hexmac>/` + meta-data + `ipxe/boot/mac-*.ipxe`), then
  `flip` + reboot (local / remote / "last steps" modes).
- **arm64 / RPi template variant** (likely just another `--template` file + a tang-server
  HostSpec variant).
- **Retiring `register-gen.py`** on the server (happens once slice three — placement &
  drive — replaces what the script does).
- **Retiring / gating the ZFS installer path.**

## Rollback
- All work is on `feat/autoinstall-renderer` in a worktree; new modules are additive and
  not wired into any existing flow except a new subcommand. Revert = drop the branch /
  `git worktree remove`. No existing behavior changes, so nothing to undo on `main`.
