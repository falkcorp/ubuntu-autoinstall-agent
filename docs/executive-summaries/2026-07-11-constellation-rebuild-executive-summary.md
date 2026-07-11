<!-- file: docs/executive-summaries/2026-07-11-constellation-rebuild-executive-summary.md -->
<!-- version: 1.0.0 -->
<!-- guid: 6f199997-416a-4ec7-9949-c506a6f9d5b4 -->
<!-- last-edited: 2026-07-11 -->

# Executive Summary: Constellation Rebuild — Wave 1 Execution

**Shipped:** PRs [#52–#75](https://github.com/falkcorp/ubuntu-autoinstall-agent/pulls?q=is%3Apr+is%3Amerged+merged%3A2026-07-10..2026-07-11), covering 2026-07-10 through 2026-07-11 (24 merged; #75 was documentation only and is not broken out below)
**Related doc:** [../status/2026-07-11-constellation-rebuild-wave1.md](../status/2026-07-11-constellation-rebuild-wave1.md) — internal status report with the per-PR table, in-flight branches, and next-session resume steps.

This covers the first execution wave of a planned rebuild of the fleet's
Ubuntu-installation system: replacing a single unguarded network server with
several small, purpose-built services that talk to each other. Each theme
below groups a related set of changes; the most important pull request
numbers are named inline as evidence.

## Executive Summary

- **A new five-piece software foundation, with nothing broken along the
  way.** The codebase was split from one monolithic program into five
  separate, independently buildable pieces — a shared code library, a
  command-line tool, a network-protocol layer, and the first two of several
  planned background services — while keeping every existing feature
  working throughout (#52, #57, #58, #64, #65; automated build checks
  extended in #53, #54). The first pieces of a new web-based dashboard for
  operators were also scaffolded (#61).
- **Fragile, untested setup scripts replaced with tested code.** Four
  scripts used to prepare install media, inject configuration secrets,
  build disk images, and pre-validate a build before it touches real
  hardware were rewritten from shell scripts into tested code (#59, #60,
  #62, #63). The original scripts are untouched and still work — this is a
  safety-net replacement, not a cutover.
- **Remote power control completed for the whole fleet.** Two more ways to
  remotely power-cycle a machine (using each manufacturer's built-in
  remote-management hardware) were added alongside the method already in
  place, so every machine in the fleet now has a supported remote power
  option (#68, #71).
- **Disk-encryption key management made safer.** New tooling manages the
  physical security keys used to unlock encrypted disks, including a
  safeguard that refuses to remove a key if doing so would leave a machine
  with too few working ways to unlock itself (#55, #67).
- **A new record-keeping service, matching everything the old server does.**
  Built the first always-on background service, which owns the fleet's
  official record of which machines exist and their status — designed to
  keep working even if its database goes down temporarily — and brought it
  to full behavioral parity with every feature of the server it will
  eventually replace (#69, #72, #74).
- **Login, permissions, and a tamper-evident activity log, for the first
  time.** The old server had no login at all — anyone who could reach it on
  the network could make changes. The new system requires signing in and
  checks group membership to decide who can look versus who can act, and
  every sensitive action is now recorded in an activity log designed so
  that tampering with a past entry would be detectable (#72).
- **Automated, safe multi-step workflows.** Approving a new machine and
  reinstalling an existing one both involve several dependent steps run in
  the right order, with automatic clean-up if any step fails partway
  through — replacing what would otherwise be manual, error-prone
  processes (#72, #74).
- **A new way for machines to prove their identity.** Machines can now be
  issued a cryptographic certificate that proves who they are to the
  fleet's services, instead of relying only on their network address (which
  can be spoofed) — the plumbing an approved machine will use to talk to
  the rest of the new system going forward (#70, #74).
- **Security review caught and fixed six issues before anything shipped to
  a live server.** Every one of these was found by automated review of the
  new code itself — not by an external report or an incident — and every
  one is fixed and merged in this same wave (#56, #66, #73, #74; see
  Highest-risk items below).

Verification note: every change above is covered by automated tests written
alongside it (the test count grew from 311 to 560 over the course of this
wave with zero failures at any point), and the security fixes each carry a
dedicated test proving the specific issue can no longer occur.

**Highest-risk items this wave** — the ones a stakeholder most needs to know
about, because each one touched security or could have caused data loss or
unauthorized access before it was caught:

- **#56** — a disk-encryption device path was checked only loosely before
  being used to build a system command, which could theoretically have let
  a crafted value smuggle in extra unintended commands; fixed by strictly
  limiting it to the characters a real device path can contain.
- **#66** — the same class of issue, found in the pre-hardware validation
  harness's command-building code and in a line that prints a disk-unlock
  passphrase; fixed by properly quoting every value before it's used in a
  command.
- **#73 (path traversal)** — a network message the system processes
  automatically could have been crafted to make the server write a file
  *outside* its intended folder, potentially overwriting an unrelated file
  on the server; fixed by strictly sanitizing the filename before use.
- **#73 (login-session hijacking)** — the new sign-in flow didn't fully
  verify that a login attempt was coming from the same browser that started
  it, the class of gap attackers use to trick someone into completing a
  login they didn't initiate; closed by binding the login attempt to the
  browser with a short-lived, protected cookie.
- **#74 (argument smuggling)** — a machine's hostname was passed as a plain
  argument to a certificate-generation tool; a hostname crafted to look
  like a command-line option could have redirected where that tool wrote
  its output. Fixed by rejecting anything that isn't a plain, literal
  hostname before it's used.
- **#74 (certificate-renewal identity check)** — the logic for renewing a
  machine's identity certificate would have re-issued it using
  attacker-suppliable identity information sent along with the renewal
  request, rather than the identity that was originally verified when the
  certificate was first approved; this was found and fixed during the same
  review that produced the feature, before it ever merged.

## What changed, in plain terms

### 1. A new five-piece software foundation

**What it was:** The system started the day as one large program.
Everything — the command-line tool, the eventual background services, the
low-level utility code — lived in a single undivided codebase, which makes
it harder to build, test, and run pieces independently as more services get
added.

**Why it mattered:** A rebuild that adds several new background services
needs each one to be buildable, testable, and deployable on its own. Doing
that inside one undivided codebase gets harder and riskier the more that's
piled into it — every change risks touching everything else.

**The fix:** The codebase was split into five separate pieces — with the
split itself verified to change nothing about existing behavior (the exact
same automated tests passed before and after) — and the shared network
protocol, device-discovery, fleet-configuration, and self-update layers
that every future service will need were built and tested. The first
pieces of a new web-based operator dashboard were also scaffolded
alongside this work.

### 2. Legacy setup scripts replaced with tested code

**What it was:** Four operational scripts — for preparing bootable install
media, injecting real secrets into a machine's configuration at
install-time, building custom disk images, and running a full
pre-validation check in a virtual machine before ever touching real
hardware — existed only as shell scripts with no automated tests.

**Why it mattered:** Untested shell scripts are one of the riskiest places
for a bug to hide, especially the one that injects real secrets into a
configuration file — a mistake there could leak a credential or place it
somewhere it shouldn't be. And a script with no test coverage can silently
break the day the tool it depends on changes behavior.

**The fix:** All four were rewritten as tested code with the exact same
capabilities, including every safety check the original secret-injection
script had (refusing to run against an untrusted location, verifying file
permissions, refusing to leave a placeholder value unfilled). The original
scripts are left in place and untouched — nothing has switched over to the
new versions yet.

### 3. Remote power control completed

**What it was:** Only one method of remotely powering a machine on or off
was supported; two other classes of machine in the fleet use different
manufacturers' built-in remote-management hardware that wasn't wired up
yet.

**Why it mattered:** Without remote power control, recovering a
non-responsive machine requires physically walking over to it — not always
practical, and not something that can be automated as part of a
reinstall workflow.

**The fix:** Support for the two remaining manufacturers' remote-management
hardware was added, each going through the same fallback logic (try the
preferred tool first, fall back to a lower-level protocol) and validated
against a simulated device rather than real hardware.

### 4. Disk-encryption key management made safer

**What it was:** The fleet's machines use encrypted disks unlocked by
physical security keys. There was no tooling to enroll a new key or safely
retire an old one, and no automated check preventing an operator from
accidentally removing the last remaining way to unlock a machine.

**Why it mattered:** An encrypted disk with no working unlock method is
equivalent to a permanently locked one — the data is not recoverable. A
key-rotation mistake on a live, encrypted server is exactly the kind of
error that's easy to make by hand and catastrophic when it happens.

**The fix:** New commands enroll and safely retire keys, always adding the
replacement key and confirming it works *before* removing the old one
(never the reverse), and a quorum check refuses to proceed if doing so
would leave a machine with fewer than the required minimum number of
working unlock methods — with an explicit, deliberate override required to
bypass it.

### 5. A new record-keeping service, matching the old server

**What it was:** The fleet's authoritative record of which machines exist,
their approval status, and their install history has always lived inside
the same unguarded server described in the introduction. This wave built
the first of several new background services to own that record instead.

**Why it mattered:** A record-keeping service that can't tell the
difference between "the database is briefly unreachable" and "the record
is gone" risks either losing track of machines during a hiccup or refusing
to function at all until the database comes back. And a replacement
service is only safe to switch over to once it does everything the old one
does — nothing less.

**The fix:** The new service falls back to a local backup copy of the
record when its database is temporarily unreachable, so routine hiccups
don't take the whole system down, while still refusing certain sensitive
actions (like approving a new machine) until the real database is back.
Every feature the legacy server exposes — machine registration, health
check-ins, install-completion reporting, machine approval and removal, and
more — was then re-implemented in the new service to match, a required
step before the old server can eventually be retired.

### 6. Login, permissions, and a tamper-evident activity log

**What it was:** The old server had no concept of a logged-in user — anyone
who could reach it over the network could approve a new machine or change
its configuration, with no record of who did it.

**Why it mattered:** Unauthenticated write access on infrastructure that
provisions and reconfigures physical servers is a real exposure: anyone
with network access, intentionally or by mistake, could take an action
with no accountability trail.

**The fix:** The new operator plane requires signing in with an external
identity provider and checks group membership to decide who can only view
versus who can make changes, defaulting to the most restrictive option if
that check itself can't be completed. Every sensitive action now writes an
entry to an activity log where each entry is cryptographically linked to
the one before it, so an attempt to quietly delete or alter a past entry
would be detectable.

### 7. Automated, safe multi-step workflows

**What it was:** Two common operator actions — approving a newly
discovered machine, and triggering a full reinstall of an existing one —
each require several dependent steps to happen in a specific order across
multiple services (placing configuration files, updating network boot
settings, updating the official record), with no existing automation to do
that safely.

**Why it mattered:** If a multi-step process like this is interrupted
partway through by a failure in one step, the system can be left in an
inconsistent state — for example, a machine that's been told to boot into
an install process but was never actually given the configuration it
needs.

**The fix:** Both workflows now run through a coordinator that performs
the steps in a fixed, safe order (placing configuration before activating
it, never the reverse) and automatically undoes completed steps in reverse
order if a later step fails, rather than leaving things half-done. A
reinstall that doesn't complete within an expected time window is
automatically rolled back to a safe state instead of being left hanging
indefinitely.

### 8. A new way for machines to prove their identity

**What it was:** Historically, the only way the system recognized a
machine was by its network address — which can be spoofed by anything else
on the same local network. There was no way for a machine to
cryptographically prove it was who it claimed to be.

**Why it mattered:** Network-address-based identity is not a real security
boundary; anything sharing the network segment can impersonate a known
machine. As more services come online that trust a machine's identity to
make decisions, that gap becomes a bigger liability.

**The fix:** A new certificate-issuing service lets a machine generate its
own private key, request a certificate, and — after an operator manually
approves the request — receive a signed certificate it can use to prove
its identity going forward, including automatically renewing that
certificate before it expires without needing another manual approval,
as long as the request is still coming from the same machine that was
originally approved.

### 9. Security hardening — six issues caught same-day

**What it was:** As each new piece of code merged, it went through an
automated security review before being treated as done. That review
surfaced six distinct issues across four pull requests — ranging from
loosely-validated values flowing into system commands, to a gap in how
login sessions were verified, to a certificate-renewal path that would
have trusted unverified information on resubmission.

**Why it mattered:** Every one of these issues, left unfixed, would have
been a real weakness in a system that's specifically being built to be
more secure than what it replaces — the entire point of this rebuild.
Catching them in the same session they were introduced, before any of this
code runs on a live server, is exactly what the review process is for.

**The fix:** Each issue was fixed the same day it was found, with a
dedicated test added proving the specific issue can no longer occur, and
merged as part of the same wave rather than left as follow-up work. See
the Highest-risk items list above for the specific issues.
