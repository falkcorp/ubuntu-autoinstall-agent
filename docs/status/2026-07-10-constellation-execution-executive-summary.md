<!-- file: docs/status/2026-07-10-constellation-execution-executive-summary.md -->
<!-- version: 1.0.0 -->
<!-- guid: 35fca813-3669-4790-975b-8f17493ef734 -->
<!-- last-edited: 2026-07-10 -->

# Executive Summary: Constellation Rebuild — Execution Wave 1

**Shipped:** PRs [#52–#74](https://github.com/falkcorp/ubuntu-autoinstall-agent/pulls?q=is%3Apr+is%3Amerged+merged%3A2026-07-10..2026-07-10) (23 merged), plus 2 branches pushed but not yet merged (untested)
**Related doc:** [../constellation/EXECUTION-STATUS-2026-07-10.md](../constellation/EXECUTION-STATUS-2026-07-10.md) — full technical breakdown: every PR, every in-flight branch, every follow-up item, and exact resume instructions.

This is the first execution wave of the constellation rebuild plan (the design
was approved and task-briefed earlier the same day). Instead of one server
running everything, the fleet's install system is becoming several small,
focused Rust programs that talk to each other — replacing a single unguarded
Python server that anyone on the network could talk to with no login and no
record of who did what.

## Executive Summary

- **The rebuild's foundation is in and tested.** Converted the codebase from
  one monolithic program into five separate, independently buildable pieces
  (a shared library, a command-line tool, a network-protocol layer, and the
  first two of several planned background services), without breaking any
  existing functionality — 560 automated tests pass, up from 311 at the
  start of the day, and every single change so far is purely additive: the
  old system is untouched and still running.
- **New background service: the registry.** Built the first of several
  planned always-on services — this one owns the fleet's official
  record-keeping (which machines exist, their status, install history). It's
  designed to keep working even if its database goes down temporarily,
  falling back to a local backup file rather than simply failing.
- **Login and permissions, for the first time.** The old system had no login
  at all — anyone who could reach it on the network could approve or
  reconfigure a machine. The new system requires signing in with a GitHub
  account and checks team membership to decide who can just look versus who
  can make changes.
- **A tamper-evident activity log.** Every sensitive action (approvals,
  configuration changes) now gets recorded in a log where each entry is
  cryptographically linked to the one before it — so if someone tried to
  quietly delete or alter a log entry after the fact, that tampering would
  be detectable.
- **Safer disk-encryption key management.** Added tooling to enroll and
  rotate the physical security keys (YubiKeys) that unlock encrypted disks,
  including a safety guard that refuses to remove a key if doing so would
  leave fewer than the minimum number of working unlock methods on a
  machine — preventing an operator from accidentally locking themselves out
  of an encrypted server.
- **Remote power control, completed.** Two more ways to remotely power
  cycle a machine (AMD's and Intel's built-in remote-management chips) were
  added alongside the existing method, rounding out the fleet's remote
  power options.
- **Legacy shell scripts replaced with tested code.** Several fragile,
  untested shell scripts (building install USB drives, injecting secrets
  into configs, building disk images, and the pre-deployment validation
  test harness) were rewritten as tested Rust code — the old scripts still
  work and haven't been deleted yet, this is a safety-net replacement, not
  a cutover.
- **Full feature-parity work on the old server's endpoints.** Every network
  endpoint the legacy Python server exposes (machine registration, health
  check-ins, install-completion webhooks, machine approval/removal, and
  more) has been re-implemented in the new system with matching behavior,
  as a required step before the old server can eventually be retired.
- **A new secure-identity system for machines**, so a machine can prove who
  it is to the fleet's services using a certificate instead of just its
  network address (which can be spoofed) — this is the plumbing an approved
  machine will use to talk to the fleet's other new services going forward.

## Highest-risk items — caught and fixed same-day

All four were found by automated security review of the new code, before
anything shipped to a live server, and are fixed and merged:

- A **write-anywhere bug**: a network message the system automatically
  processes could have been crafted to make the server write a file
  *outside* its intended log folder — potentially overwriting other files
  on the server. Fixed by strictly sanitizing the filename before use.
- A **login-hijacking gap**: the new GitHub-login flow didn't fully verify
  that a login attempt was coming from the same browser that started it,
  which is the class of bug attackers use to trick someone into logging
  into an attacker-controlled session. Closed by binding the login attempt
  to the browser with a short-lived, protected cookie.
- A **command-injection bug**: a disk-encryption device path was being
  built into a system command without being strictly validated, which
  could theoretically let a crafted value smuggle in extra unintended
  commands. Fixed by restricting it to only the characters a real device
  path can contain.
- An **argument-smuggling bug**: a machine's hostname was being passed
  as a plain argument to a certificate-generation tool; a hostname crafted
  to look like a command-line flag could have redirected where that tool
  wrote its output. Fixed by rejecting anything that isn't a plain,
  literal hostname before it's used.

## What's still in flight

Five more pieces of this wave were started; two have code pushed but
**not yet verified or merged** (a dashboard/reporting page and a
certificate-embedding change), and three didn't get far enough to leave
anything usable. None of this is on a live server or affects the currently
running system — see the linked technical doc for exact branch names and
resume steps.
