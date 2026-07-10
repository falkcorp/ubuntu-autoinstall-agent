// file: crates/uaa-control/src/reinstall.rs
// version: 1.1.0
// guid: 27c0208b-9b94-40e4-b60b-27732d25e471
// last-edited: 2026-07-10

//! One-click reinstall flow + boot-target reconciliation (spec C3 / Decision 13).
//!
//! `boot_target` on the `machines` registry row is the ONE authoritative field.
//! [`reinstall_machine`] sets it to `custom-autoinstall`, projects that value to
//! BOTH the uaa-web iPXE flip layer and the uaa-pxe dnsmasq layer, power-cycles
//! the host (off then on — reset/cycle are unrepresentable, Decision 15), then
//! watches install events for a bounded window. A host must never be left
//! half-projected: if either layer fails to reconcile, both are flipped back to
//! `local-disk` best-effort and the registry field is restored. If the bounded
//! watch times out (or the install itself reports failure), the same fail-safe
//! flip-back runs and a loud `tracing::error!` alert is emitted. A cooldown
//! window refuses an un-confirmed re-trigger shortly after a prior attempt.
//!
//! Hard refusals (fail-closed, BEFORE any side effect): the `FleetConfig`
//! reinstall deny-list (`unimatrixone`) and any host whose registry approval
//! status is not `approved`.
//!
//! # Coordinator wiring (read before touching any other file)
//!
//! This module is purely additive and self-contained. It does NOT import from
//! `saga.rs` (CT-05) or `db::registry` (CT-02) because both were still
//! header-only stubs at authoring time (wave-4 gate: only CT-01 + core-proto
//! CP-03 were merged). Per the coordinator wiring rule this module declares its
//! own narrow local traits instead of blocking:
//!
//! - [`WebClient`] / [`PxeClient`] — TODO(coordinator): once CT-05's `saga.rs`
//!   lands its own `WebClient`/`PxeClient` traits, unify these two
//!   declarations (either re-export one from the other, or hoist a shared
//!   definition into `db::mod` / a new `clients` module). Until then the two
//!   declarations are structurally identical and safe to keep separate.
//! - [`ReinstallRegistry`] — a narrow registry seam (get/set boot_target,
//!   approval status, cooldown timestamp) standing in for CT-02's `db::registry`
//!   `RegistryStore`. TODO(coordinator): once CT-02 lands, wire a
//!   `RegistryStore`-backed impl of `ReinstallRegistry` (construction only).
//!
//! [`PowerControl`] wraps `uaa_core::power::run_power_action` — the mock impl
//! used by every test never touches SSH or a real executor; [`RuntimePowerControl`]
//! is the only impl that does, and it is pure construction/delegation (no IPMI
//! logic lives here — see `uaa-core/src/power/mod.rs`).

use std::time::{Duration, SystemTime};

use async_trait::async_trait;

use uaa_core::fleet::FleetConfig;

// ── Local narrow traits (seams) ─────────────────────────────────────────────

/// uaa-web iPXE boot-target flip client. Local stand-in for CT-05's trait of
/// the same shape — see the module-level coordinator-wiring note.
#[async_trait]
pub trait WebClient {
    /// Flip the iPXE boot target for `mac` to `target` (a `BootTarget` wire
    /// string: `"custom-autoinstall"` or `"local-disk"`).
    async fn flip_boot_target(&self, mac: &str, target: &str) -> anyhow::Result<()>;
}

/// uaa-pxe dnsmasq boot-target client. Local stand-in for CT-05's trait of the
/// same shape — see the module-level coordinator-wiring note.
#[async_trait]
pub trait PxeClient {
    /// Set the dnsmasq/PXE boot target for `mac` to `target` (same wire
    /// strings as [`WebClient::flip_boot_target`]).
    async fn set_boot_target(&self, mac: &str, target: &str) -> anyhow::Result<()>;
}

/// Remote power control, keyed by hostname. Wraps
/// `uaa_core::power::run_power_action` (Decision 15: ipmitool always runs via
/// `ssh 172.16.2.30`, never locally). Only `off`/`on` are representable —
/// `chassis power reset`/`cycle` are unreliable on the X10DSC+ and stay
/// unrepresentable at every layer, including this one.
///
/// `?Send`: `run_power_action` takes `&mut dyn CommandExecutor`, a trait that
/// does not itself require `Send`, so [`RuntimePowerControl`]'s futures are
/// not `Send`. This module never spawns a task across threads, so opting out
/// of async-trait's default `Send` bound is the correct (and only) fix
/// available without touching `uaa-core` (out of scope for this file).
#[async_trait(?Send)]
pub trait PowerControl {
    /// Power the host off.
    async fn off(&self, host: &str) -> anyhow::Result<()>;
    /// Power the host on.
    async fn on(&self, host: &str) -> anyhow::Result<()>;
}

/// Install-event status as reported by the installer's checkin/webhook path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallStatus {
    /// The install completed successfully.
    Success,
    /// The install reported a failure.
    Failed,
    /// The install is still running (or no event has arrived yet).
    InProgress,
}

/// Narrow seam over the install-event source polled by the bounded watch.
#[async_trait]
pub trait InstallWatch {
    /// Latest known install status for `mac`, or `None` if no event has been
    /// recorded yet.
    async fn latest_status(&self, mac: &str) -> anyhow::Result<Option<InstallStatus>>;
}

/// Registry seam: everything [`reinstall_machine`] needs to read/write on the
/// `machines` row, without depending on CT-02's not-yet-merged `db::registry`.
#[async_trait]
pub trait ReinstallRegistry {
    /// Look up the machine's hostname, approval, current `boot_target`, and
    /// cooldown timestamp. `None` means the mac is not registered at all —
    /// treated identically to `NotApproved` (fail-closed; there is no
    /// approval status to check).
    async fn get_machine(&self, mac: &str) -> anyhow::Result<Option<RegistryMachine>>;
    /// Persist a new `boot_target` for `mac` (used both for the forward write
    /// and for restore-on-failure / fail-safe flip-back).
    async fn set_boot_target(&self, mac: &str, target: &str) -> anyhow::Result<()>;
    /// Stamp the cooldown timestamp for `mac`.
    async fn stamp_reinstall(&self, mac: &str, at: SystemTime) -> anyhow::Result<()>;
}

/// The subset of a `machines` row [`ReinstallRegistry::get_machine`] needs to
/// return. Deliberately NOT `db::MachineRow` (owned by CT-01; this seam is
/// independent of that schema until CT-02 lands — see coordinator-wiring note).
#[derive(Debug, Clone, PartialEq)]
pub struct RegistryMachine {
    pub hostname: String,
    pub approved: bool,
    pub boot_target: String,
    pub last_reinstall_at: Option<SystemTime>,
}

/// Injectable clock: `now()` for cooldown/timeout comparisons, `sleep()` for
/// the watch poll interval. The real impl sleeps for real; the test impl
/// advances a virtual clock instantly so tests never wait on a real timer.
#[async_trait]
pub trait Clock {
    /// Current time.
    fn now(&self) -> SystemTime;
    /// Wait `dur` before the next watch poll.
    async fn sleep(&self, dur: Duration);
}

/// Real-time [`Clock`] impl used outside tests.
pub struct SystemClock;

#[async_trait]
impl Clock for SystemClock {
    fn now(&self) -> SystemTime {
        SystemTime::now()
    }

    async fn sleep(&self, dur: Duration) {
        tokio::time::sleep(dur).await;
    }
}

// ── Request/outcome/refusal types ───────────────────────────────────────────

/// A one-click reinstall request.
#[derive(Debug, Clone)]
pub struct ReinstallRequest {
    pub mac: String,
    /// Must be `true` to bypass an active cooldown window.
    pub confirm: bool,
}

/// Result of a [`reinstall_machine`] call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReinstallOutcome {
    /// The install reported success.
    Done,
    /// The bounded watch timed out, or the install reported failure; a
    /// fail-safe flip-back to `local-disk` on both layers (+ registry) was
    /// attempted. `flip_back_ok` is `false` if any leg of that flip-back
    /// itself failed (never swallowed — also `tracing::error!`-logged).
    TimedOutFlippedBack { flip_back_ok: bool },
    /// A guard refused the request before any side effect.
    Refused(RefusalReason),
}

/// Why a reinstall request was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefusalReason {
    /// The host is on the `FleetConfig` reinstall deny-list (e.g. `unimatrixone`).
    DenyListed,
    /// The mac is unregistered, or its registry status is not `approved`.
    NotApproved,
    /// A prior reinstall attempt is within the cooldown window and the
    /// request did not set `confirm: true`. Names the remaining window.
    CooldownActive { remaining_min: u64 },
    /// The dual-layer projection could not be reconciled; both layers (and
    /// the registry) were flipped/restored best-effort. Names the underlying
    /// web/pxe errors.
    Unreconciled(String),
}

/// Timing configuration for [`reinstall_machine`].
#[derive(Debug, Clone)]
pub struct ReinstallConfig {
    /// How long the watch polls before declaring a timeout (default 45 min).
    pub watch_timeout: Duration,
    /// Delay between watch polls (default 30 s).
    pub poll_interval: Duration,
    /// Cooldown window after an attempt reaches the power step before a
    /// re-trigger requires `confirm: true` (default 30 min).
    pub cooldown: Duration,
}

impl Default for ReinstallConfig {
    fn default() -> Self {
        Self {
            watch_timeout: Duration::from_secs(45 * 60),
            poll_interval: Duration::from_secs(30),
            cooldown: Duration::from_secs(30 * 60),
        }
    }
}

/// Bundles every seam [`reinstall_machine`] depends on. All trait objects so
/// tests inject mocks; `handle_reinstall` is the only caller that constructs
/// the real [`RuntimePowerControl`].
pub struct ReinstallDeps<'a> {
    pub web: &'a dyn WebClient,
    pub pxe: &'a dyn PxeClient,
    pub power: &'a dyn PowerControl,
    pub watch: &'a dyn InstallWatch,
    pub registry: &'a dyn ReinstallRegistry,
    pub fleet: &'a FleetConfig,
    pub clock: &'a dyn Clock,
    pub config: ReinstallConfig,
}

const LOCAL_DISK: &str = "local-disk";
const CUSTOM_AUTOINSTALL: &str = "custom-autoinstall";

// ── Driver ───────────────────────────────────────────────────────────────────

/// Drive one one-click reinstall attempt per spec C3 / Decision 13.
///
/// Sequence (spelled out per the brief, mirrored in the module docs above):
/// (1) guards — deny-list, not-approved, cooldown-without-confirm — each
/// returns before any mutating call; (2) registry write `boot_target =
/// custom-autoinstall`; (3) dual projection (web + pxe); on a single-layer
/// failure, both are flipped back best-effort, the registry field is
/// restored, and `Unreconciled` is returned — power is never invoked; (4)
/// power off then on (never reset/cycle); (5) bounded watch — success returns
/// `Done`, failure/timeout runs the fail-safe flip-back and returns
/// `TimedOutFlippedBack`. Every attempt that reaches the power step stamps
/// the cooldown timestamp, regardless of the eventual watch outcome.
pub async fn reinstall_machine(
    deps: &ReinstallDeps<'_>,
    req: ReinstallRequest,
) -> anyhow::Result<ReinstallOutcome> {
    let mac = req.mac.as_str();

    // A registry read is not a side effect (the Goal's "before any side
    // effect" refers to mutations) — but it must happen before any guard can
    // be evaluated, since the deny-list is hostname-keyed and approval lives
    // on the row.
    let machine = deps.registry.get_machine(mac).await?;
    let machine = match machine {
        Some(m) => m,
        None => return Ok(ReinstallOutcome::Refused(RefusalReason::NotApproved)),
    };

    // Guard 1: deny-list, sourced from FleetConfig — never hardcoded here.
    if deps
        .fleet
        .reinstall_deny
        .iter()
        .any(|h| h == &machine.hostname)
    {
        return Ok(ReinstallOutcome::Refused(RefusalReason::DenyListed));
    }

    // Guard 2: approval status.
    if !machine.approved {
        return Ok(ReinstallOutcome::Refused(RefusalReason::NotApproved));
    }

    // Guard 3: cooldown, unless explicitly confirmed.
    if let Some(last) = machine.last_reinstall_at {
        let elapsed = deps.clock.now().duration_since(last).unwrap_or_default();
        if elapsed < deps.config.cooldown && !req.confirm {
            let remaining = deps.config.cooldown - elapsed;
            let remaining_min = remaining.as_secs().div_ceil(60);
            return Ok(ReinstallOutcome::Refused(RefusalReason::CooldownActive {
                remaining_min,
            }));
        }
    }

    // Guards passed. Remember the prior boot_target so a reconciliation
    // failure can restore it exactly.
    let prior_boot_target = machine.boot_target.clone();

    // Registry write: boot_target = custom-autoinstall (the single
    // authoritative field).
    deps.registry.set_boot_target(mac, CUSTOM_AUTOINSTALL).await?;

    // Dual projection.
    let web_res = deps.web.flip_boot_target(mac, CUSTOM_AUTOINSTALL).await;
    let pxe_res = deps.pxe.set_boot_target(mac, CUSTOM_AUTOINSTALL).await;

    if web_res.is_err() || pxe_res.is_err() {
        // Never-half-projected: flip back whichever layer(s) actually
        // succeeded forward (a layer that already errored never got
        // projected, so there is nothing to undo there), restore the
        // registry field, and refuse with the reconciliation error. Power is
        // NOT invoked on this path.
        let mut flip_back_errors = Vec::new();

        if web_res.is_ok() {
            if let Err(e) = deps.web.flip_boot_target(mac, LOCAL_DISK).await {
                flip_back_errors.push(format!("web flip-back failed: {e}"));
            }
        }
        if pxe_res.is_ok() {
            if let Err(e) = deps.pxe.set_boot_target(mac, LOCAL_DISK).await {
                flip_back_errors.push(format!("pxe flip-back failed: {e}"));
            }
        }
        if let Err(e) = deps.registry.set_boot_target(mac, &prior_boot_target).await {
            flip_back_errors.push(format!("registry restore failed: {e}"));
        }

        for err in &flip_back_errors {
            tracing::error!("reinstall reconciliation flip-back: {err}");
        }

        let reason = format!(
            "boot_target reconciliation failed for {mac} ({}): web={:?} pxe={:?}",
            machine.hostname,
            web_res.as_ref().err().map(|e| e.to_string()),
            pxe_res.as_ref().err().map(|e| e.to_string()),
        );
        tracing::error!("{reason}");
        return Ok(ReinstallOutcome::Refused(RefusalReason::Unreconciled(reason)));
    }

    // Power cycle: off then on, explicitly — reset/cycle are unrepresentable.
    deps.power.off(&machine.hostname).await?;
    deps.power.on(&machine.hostname).await?;

    // Every attempt that reached the power step stamps the cooldown
    // timestamp, regardless of the eventual watch outcome.
    deps.registry.stamp_reinstall(mac, deps.clock.now()).await?;

    // Bounded watch.
    let start = deps.clock.now();
    loop {
        if let Some(status) = deps.watch.latest_status(mac).await? {
            match status {
                InstallStatus::Success => return Ok(ReinstallOutcome::Done),
                InstallStatus::Failed => {
                    return Ok(fail_safe_flip_back(deps, mac, &machine.hostname).await);
                }
                InstallStatus::InProgress => {}
            }
        }

        let elapsed = deps.clock.now().duration_since(start).unwrap_or_default();
        if elapsed >= deps.config.watch_timeout {
            return Ok(fail_safe_flip_back(deps, mac, &machine.hostname).await);
        }

        deps.clock.sleep(deps.config.poll_interval).await;
    }
}

/// Fail-safe: flip both layers (and the registry) back to `local-disk`
/// best-effort, loudly alert, and report whether the flip-back itself fully
/// succeeded. Never swallows a flip-back failure — every leg's error is
/// `tracing::error!`-logged AND folded into `flip_back_ok`.
async fn fail_safe_flip_back(
    deps: &ReinstallDeps<'_>,
    mac: &str,
    hostname: &str,
) -> ReinstallOutcome {
    let web_res = deps.web.flip_boot_target(mac, LOCAL_DISK).await;
    let pxe_res = deps.pxe.set_boot_target(mac, LOCAL_DISK).await;
    let registry_res = deps.registry.set_boot_target(mac, LOCAL_DISK).await;

    if let Err(e) = &web_res {
        tracing::error!("fail-safe flip-back: web layer failed for {hostname} ({mac}): {e}");
    }
    if let Err(e) = &pxe_res {
        tracing::error!("fail-safe flip-back: pxe layer failed for {hostname} ({mac}): {e}");
    }
    if let Err(e) = &registry_res {
        tracing::error!(
            "fail-safe flip-back: registry restore failed for {hostname} ({mac}): {e}"
        );
    }

    let flip_back_ok = web_res.is_ok() && pxe_res.is_ok() && registry_res.is_ok();
    tracing::error!(
        "ALERT: reinstall watch bounded-timeout/failure for {hostname} ({mac}); fail-safe \
         flip-back {}",
        if flip_back_ok {
            "succeeded"
        } else {
            "PARTIALLY FAILED — manual intervention required"
        }
    );

    ReinstallOutcome::TimedOutFlippedBack { flip_back_ok }
}

// ── Runtime power impl + thin entry point ───────────────────────────────────

/// Real [`PowerControl`] impl: pure construction/delegation to
/// `uaa_core::power::run_power_action` — no IPMI logic lives here. Every call
/// opens a fresh SSH session to `POWER_SERVER` (172.16.2.30), matching the
/// existing CLI `uaa power` command's pattern.
pub struct RuntimePowerControl {
    ipmi_password: Option<String>,
}

impl RuntimePowerControl {
    pub fn new(ipmi_password: Option<String>) -> Self {
        Self { ipmi_password }
    }

    async fn run(&self, host: &str, action: uaa_core::power::PowerAction) -> anyhow::Result<()> {
        use uaa_core::network::SshClient;
        use uaa_core::power::{run_power_action, POWER_SERVER};

        let mut client = SshClient::new();
        client.connect(POWER_SERVER, "jdfalk").await?;
        run_power_action(&mut client, host, action, self.ipmi_password.as_deref()).await?;
        Ok(())
    }
}

#[async_trait(?Send)]
impl PowerControl for RuntimePowerControl {
    async fn off(&self, host: &str) -> anyhow::Result<()> {
        self.run(host, uaa_core::power::PowerAction::Off).await
    }

    async fn on(&self, host: &str) -> anyhow::Result<()> {
        self.run(host, uaa_core::power::PowerAction::On).await
    }
}

/// Thin entry point the operator plane (CT-07) and gRPC layer will call.
/// Construction only: builds the real [`RuntimePowerControl`] and delegates
/// straight to [`reinstall_machine`] — no orchestration logic lives here.
#[allow(clippy::too_many_arguments)]
pub async fn handle_reinstall(
    registry: &dyn ReinstallRegistry,
    web: &dyn WebClient,
    pxe: &dyn PxeClient,
    watch: &dyn InstallWatch,
    fleet: &FleetConfig,
    clock: &dyn Clock,
    config: ReinstallConfig,
    ipmi_password: Option<String>,
    req: ReinstallRequest,
) -> anyhow::Result<ReinstallOutcome> {
    let power = RuntimePowerControl::new(ipmi_password);
    let deps = ReinstallDeps {
        web,
        pxe,
        power: &power,
        watch,
        registry,
        fleet,
        clock,
        config,
    };
    reinstall_machine(&deps, req).await
}

// ── Unit tests (all-mock, tick clock, zero hardware/SSH/network) ───────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{HashMap, VecDeque};
    use std::sync::{Arc, Mutex};

    const TEST_MAC: &str = "aa:bb:cc:dd:ee:ff";
    const TEST_HOST: &str = "len-serv-999";

    /// Shared, cloneable call-log: multiple mocks can be handed the same
    /// `CallLog` (cloning only the inner `Arc`) so a single ordered sequence
    /// can be asserted across every seam, or each mock can get its own fresh
    /// log for isolated zero-call assertions.
    #[derive(Clone, Default)]
    struct CallLog(Arc<Mutex<Vec<String>>>);

    impl CallLog {
        fn push(&self, s: impl Into<String>) {
            self.0.lock().unwrap().push(s.into());
        }

        fn calls(&self) -> Vec<String> {
            self.0.lock().unwrap().clone()
        }
    }

    struct MockWeb {
        log: CallLog,
        fail_on: Vec<String>,
    }

    impl MockWeb {
        fn new(log: CallLog) -> Self {
            Self {
                log,
                fail_on: Vec::new(),
            }
        }
    }

    #[async_trait]
    impl WebClient for MockWeb {
        async fn flip_boot_target(&self, _mac: &str, target: &str) -> anyhow::Result<()> {
            self.log.push(format!("web:{target}"));
            if self.fail_on.iter().any(|t| t == target) {
                anyhow::bail!("mock web flip failure for target {target}");
            }
            Ok(())
        }
    }

    struct MockPxe {
        log: CallLog,
        fail_on: Vec<String>,
    }

    impl MockPxe {
        fn new(log: CallLog) -> Self {
            Self {
                log,
                fail_on: Vec::new(),
            }
        }

        fn failing_on(log: CallLog, target: &str) -> Self {
            Self {
                log,
                fail_on: vec![target.to_string()],
            }
        }
    }

    #[async_trait]
    impl PxeClient for MockPxe {
        async fn set_boot_target(&self, _mac: &str, target: &str) -> anyhow::Result<()> {
            self.log.push(format!("pxe:{target}"));
            if self.fail_on.iter().any(|t| t == target) {
                anyhow::bail!("mock pxe set failure for target {target}");
            }
            Ok(())
        }
    }

    struct MockPower {
        log: CallLog,
    }

    impl MockPower {
        fn new(log: CallLog) -> Self {
            Self { log }
        }
    }

    #[async_trait(?Send)]
    impl PowerControl for MockPower {
        async fn off(&self, host: &str) -> anyhow::Result<()> {
            self.log.push(format!("power:off:{host}"));
            Ok(())
        }

        async fn on(&self, host: &str) -> anyhow::Result<()> {
            self.log.push(format!("power:on:{host}"));
            Ok(())
        }
    }

    struct MockWatch {
        log: CallLog,
        responses: Mutex<VecDeque<Option<InstallStatus>>>,
        default: Option<InstallStatus>,
    }

    impl MockWatch {
        /// Always report a fixed status (or `None` forever, for a timeout test).
        fn always(log: CallLog, status: Option<InstallStatus>) -> Self {
            Self {
                log,
                responses: Mutex::new(VecDeque::new()),
                default: status,
            }
        }
    }

    #[async_trait]
    impl InstallWatch for MockWatch {
        async fn latest_status(&self, mac: &str) -> anyhow::Result<Option<InstallStatus>> {
            self.log.push(format!("watch:{mac}"));
            let mut q = self.responses.lock().unwrap();
            Ok(q.pop_front().unwrap_or_else(|| self.default.clone()))
        }
    }

    struct MockRegistry {
        machines: Mutex<HashMap<String, RegistryMachine>>,
        get_calls: CallLog,
        write_calls: CallLog,
    }

    impl MockRegistry {
        fn new(mac: &str, machine: RegistryMachine) -> Self {
            let mut m = HashMap::new();
            m.insert(mac.to_string(), machine);
            Self {
                machines: Mutex::new(m),
                get_calls: CallLog::default(),
                write_calls: CallLog::default(),
            }
        }

        fn with_shared_write_log(mac: &str, machine: RegistryMachine, write_calls: CallLog) -> Self {
            let mut m = HashMap::new();
            m.insert(mac.to_string(), machine);
            Self {
                machines: Mutex::new(m),
                get_calls: CallLog::default(),
                write_calls,
            }
        }

        fn boot_target_now(&self, mac: &str) -> String {
            self.machines
                .lock()
                .unwrap()
                .get(mac)
                .map(|m| m.boot_target.clone())
                .unwrap_or_default()
        }
    }

    #[async_trait]
    impl ReinstallRegistry for MockRegistry {
        async fn get_machine(&self, mac: &str) -> anyhow::Result<Option<RegistryMachine>> {
            self.get_calls.push(format!("get:{mac}"));
            Ok(self.machines.lock().unwrap().get(mac).cloned())
        }

        async fn set_boot_target(&self, mac: &str, target: &str) -> anyhow::Result<()> {
            self.write_calls.push(format!("registry:set_boot_target:{target}"));
            if let Some(m) = self.machines.lock().unwrap().get_mut(mac) {
                m.boot_target = target.to_string();
            }
            Ok(())
        }

        async fn stamp_reinstall(&self, mac: &str, at: SystemTime) -> anyhow::Result<()> {
            self.write_calls.push("registry:stamp_reinstall".to_string());
            if let Some(m) = self.machines.lock().unwrap().get_mut(mac) {
                m.last_reinstall_at = Some(at);
            }
            Ok(())
        }
    }

    /// Test [`Clock`]: `sleep()` advances a virtual clock instantly instead
    /// of actually waiting, so the bounded-watch tests run in microseconds.
    struct TickClock {
        now: Mutex<SystemTime>,
    }

    impl TickClock {
        fn new(start: SystemTime) -> Self {
            Self {
                now: Mutex::new(start),
            }
        }
    }

    #[async_trait]
    impl Clock for TickClock {
        fn now(&self) -> SystemTime {
            *self.now.lock().unwrap()
        }

        async fn sleep(&self, dur: Duration) {
            let mut n = self.now.lock().unwrap();
            *n += dur;
        }
    }

    fn approved_machine() -> RegistryMachine {
        RegistryMachine {
            hostname: TEST_HOST.to_string(),
            approved: true,
            boot_target: LOCAL_DISK.to_string(),
            last_reinstall_at: None,
        }
    }

    fn small_config() -> ReinstallConfig {
        ReinstallConfig {
            watch_timeout: Duration::from_secs(120),
            poll_interval: Duration::from_secs(30),
            cooldown: Duration::from_secs(30 * 60),
        }
    }

    #[tokio::test]
    async fn test_denylist_refused_zero_side_effects() {
        let fleet = FleetConfig::default(); // includes "unimatrixone" by default.
        let registry = MockRegistry::new(
            TEST_MAC,
            RegistryMachine {
                hostname: "unimatrixone".to_string(),
                approved: true,
                boot_target: LOCAL_DISK.to_string(),
                last_reinstall_at: None,
            },
        );
        let web = MockWeb::new(CallLog::default());
        let pxe = MockPxe::new(CallLog::default());
        let power = MockPower::new(CallLog::default());
        let watch = MockWatch::always(CallLog::default(), None);
        let clock = TickClock::new(SystemTime::now());

        let deps = ReinstallDeps {
            web: &web,
            pxe: &pxe,
            power: &power,
            watch: &watch,
            registry: &registry,
            fleet: &fleet,
            clock: &clock,
            config: small_config(),
        };

        let outcome = reinstall_machine(
            &deps,
            ReinstallRequest {
                mac: TEST_MAC.to_string(),
                confirm: false,
            },
        )
        .await
        .unwrap();

        assert_eq!(
            outcome,
            ReinstallOutcome::Refused(RefusalReason::DenyListed)
        );
        // Zero side effects: no mutating registry call, and every other seam
        // (which is entirely action/mutation-shaped) recorded nothing.
        assert!(registry.write_calls.calls().is_empty());
        assert!(web.log.calls().is_empty());
        assert!(pxe.log.calls().is_empty());
        assert!(power.log.calls().is_empty());
        assert!(watch.log.calls().is_empty());
    }

    #[tokio::test]
    async fn test_unapproved_refused_zero_side_effects() {
        let fleet = FleetConfig::default();
        let mut machine = approved_machine();
        machine.approved = false;
        let registry = MockRegistry::new(TEST_MAC, machine);
        let web = MockWeb::new(CallLog::default());
        let pxe = MockPxe::new(CallLog::default());
        let power = MockPower::new(CallLog::default());
        let watch = MockWatch::always(CallLog::default(), None);
        let clock = TickClock::new(SystemTime::now());

        let deps = ReinstallDeps {
            web: &web,
            pxe: &pxe,
            power: &power,
            watch: &watch,
            registry: &registry,
            fleet: &fleet,
            clock: &clock,
            config: small_config(),
        };

        let outcome = reinstall_machine(
            &deps,
            ReinstallRequest {
                mac: TEST_MAC.to_string(),
                confirm: false,
            },
        )
        .await
        .unwrap();

        assert_eq!(
            outcome,
            ReinstallOutcome::Refused(RefusalReason::NotApproved)
        );
        assert!(registry.write_calls.calls().is_empty());
        assert!(web.log.calls().is_empty());
        assert!(pxe.log.calls().is_empty());
        assert!(power.log.calls().is_empty());
        assert!(watch.log.calls().is_empty());
    }

    #[tokio::test]
    async fn test_cooldown_refused_without_confirm() {
        let fleet = FleetConfig::default();
        let start = SystemTime::now();
        let mut machine = approved_machine();
        machine.last_reinstall_at = Some(start);
        let registry = MockRegistry::new(TEST_MAC, machine);
        let web = MockWeb::new(CallLog::default());
        let pxe = MockPxe::new(CallLog::default());
        let power = MockPower::new(CallLog::default());
        let watch = MockWatch::always(CallLog::default(), None);
        // Clock is 5 minutes past last_reinstall_at; cooldown is 30 minutes.
        let clock = TickClock::new(start + Duration::from_secs(5 * 60));

        let deps = ReinstallDeps {
            web: &web,
            pxe: &pxe,
            power: &power,
            watch: &watch,
            registry: &registry,
            fleet: &fleet,
            clock: &clock,
            config: small_config(),
        };

        let outcome = reinstall_machine(
            &deps,
            ReinstallRequest {
                mac: TEST_MAC.to_string(),
                confirm: false,
            },
        )
        .await
        .unwrap();

        assert_eq!(
            outcome,
            ReinstallOutcome::Refused(RefusalReason::CooldownActive { remaining_min: 25 })
        );
        assert!(registry.write_calls.calls().is_empty());
        assert!(web.log.calls().is_empty());
        assert!(pxe.log.calls().is_empty());
        assert!(power.log.calls().is_empty());
        assert!(watch.log.calls().is_empty());
    }

    #[tokio::test]
    async fn test_cooldown_bypassed_with_confirm() {
        let fleet = FleetConfig::default();
        let start = SystemTime::now();
        let mut machine = approved_machine();
        machine.last_reinstall_at = Some(start);
        let registry = MockRegistry::new(TEST_MAC, machine);
        let web = MockWeb::new(CallLog::default());
        let pxe = MockPxe::new(CallLog::default());
        let power = MockPower::new(CallLog::default());
        let watch = MockWatch::always(CallLog::default(), Some(InstallStatus::Success));
        let clock = TickClock::new(start + Duration::from_secs(5 * 60));

        let deps = ReinstallDeps {
            web: &web,
            pxe: &pxe,
            power: &power,
            watch: &watch,
            registry: &registry,
            fleet: &fleet,
            clock: &clock,
            config: small_config(),
        };

        let outcome = reinstall_machine(
            &deps,
            ReinstallRequest {
                mac: TEST_MAC.to_string(),
                confirm: true,
            },
        )
        .await
        .unwrap();

        assert_eq!(outcome, ReinstallOutcome::Done);
        // confirm:true bypassed the cooldown guard and drove all the way through.
        assert!(!web.log.calls().is_empty());
        assert!(!pxe.log.calls().is_empty());
        assert!(!power.log.calls().is_empty());
    }

    #[tokio::test]
    async fn test_single_layer_failure_flips_back_and_restores() {
        let fleet = FleetConfig::default();
        let registry = MockRegistry::new(TEST_MAC, approved_machine());
        let web = MockWeb::new(CallLog::default());
        let pxe = MockPxe::failing_on(CallLog::default(), CUSTOM_AUTOINSTALL);
        let power = MockPower::new(CallLog::default());
        let watch = MockWatch::always(CallLog::default(), None);
        let clock = TickClock::new(SystemTime::now());

        let deps = ReinstallDeps {
            web: &web,
            pxe: &pxe,
            power: &power,
            watch: &watch,
            registry: &registry,
            fleet: &fleet,
            clock: &clock,
            config: small_config(),
        };

        let outcome = reinstall_machine(
            &deps,
            ReinstallRequest {
                mac: TEST_MAC.to_string(),
                confirm: false,
            },
        )
        .await
        .unwrap();

        match outcome {
            ReinstallOutcome::Refused(RefusalReason::Unreconciled(_)) => {}
            other => panic!("expected Unreconciled, got {other:?}"),
        }

        // web was flipped forward then back; pxe only attempted (and failed) forward.
        assert_eq!(web.log.calls(), vec!["web:custom-autoinstall", "web:local-disk"]);
        assert_eq!(pxe.log.calls(), vec!["pxe:custom-autoinstall"]);
        // Registry: forward write, then restore back to the prior value.
        assert_eq!(
            registry.write_calls.calls(),
            vec![
                "registry:set_boot_target:custom-autoinstall",
                "registry:set_boot_target:local-disk",
            ]
        );
        assert_eq!(registry.boot_target_now(TEST_MAC), LOCAL_DISK);
        // Power is never invoked on this path.
        assert!(power.log.calls().is_empty());
    }

    #[tokio::test]
    async fn test_power_off_then_on_order() {
        let fleet = FleetConfig::default();
        let registry = MockRegistry::new(TEST_MAC, approved_machine());
        let web = MockWeb::new(CallLog::default());
        let pxe = MockPxe::new(CallLog::default());
        let power = MockPower::new(CallLog::default());
        let watch = MockWatch::always(CallLog::default(), Some(InstallStatus::Success));
        let clock = TickClock::new(SystemTime::now());

        let deps = ReinstallDeps {
            web: &web,
            pxe: &pxe,
            power: &power,
            watch: &watch,
            registry: &registry,
            fleet: &fleet,
            clock: &clock,
            config: small_config(),
        };

        reinstall_machine(
            &deps,
            ReinstallRequest {
                mac: TEST_MAC.to_string(),
                confirm: false,
            },
        )
        .await
        .unwrap();

        assert_eq!(
            power.log.calls(),
            vec![
                format!("power:off:{TEST_HOST}"),
                format!("power:on:{TEST_HOST}"),
            ]
        );
    }

    #[tokio::test]
    async fn test_watch_success_done() {
        let fleet = FleetConfig::default();
        let registry = MockRegistry::new(TEST_MAC, approved_machine());
        let web = MockWeb::new(CallLog::default());
        let pxe = MockPxe::new(CallLog::default());
        let power = MockPower::new(CallLog::default());
        let watch = MockWatch::always(CallLog::default(), Some(InstallStatus::Success));
        let clock = TickClock::new(SystemTime::now());

        let deps = ReinstallDeps {
            web: &web,
            pxe: &pxe,
            power: &power,
            watch: &watch,
            registry: &registry,
            fleet: &fleet,
            clock: &clock,
            config: small_config(),
        };

        let outcome = reinstall_machine(
            &deps,
            ReinstallRequest {
                mac: TEST_MAC.to_string(),
                confirm: false,
            },
        )
        .await
        .unwrap();

        assert_eq!(outcome, ReinstallOutcome::Done);
        assert_eq!(registry.boot_target_now(TEST_MAC), CUSTOM_AUTOINSTALL);
    }

    #[tokio::test]
    async fn test_watch_timeout_flips_back_and_alerts() {
        let fleet = FleetConfig::default();
        let registry = MockRegistry::new(TEST_MAC, approved_machine());
        let web = MockWeb::new(CallLog::default());
        let pxe = MockPxe::new(CallLog::default());
        let power = MockPower::new(CallLog::default());
        // Never reports a status -> the watch loop runs out the bounded timeout.
        let watch = MockWatch::always(CallLog::default(), None);
        let clock = TickClock::new(SystemTime::now());

        let deps = ReinstallDeps {
            web: &web,
            pxe: &pxe,
            power: &power,
            watch: &watch,
            registry: &registry,
            fleet: &fleet,
            clock: &clock,
            config: small_config(), // watch_timeout = 120s, poll_interval = 30s
        };

        let outcome = reinstall_machine(
            &deps,
            ReinstallRequest {
                mac: TEST_MAC.to_string(),
                confirm: false,
            },
        )
        .await
        .unwrap();

        assert_eq!(
            outcome,
            ReinstallOutcome::TimedOutFlippedBack { flip_back_ok: true }
        );
        // Both layers re-flipped to local-disk after the forward flip.
        assert_eq!(
            web.log.calls(),
            vec!["web:custom-autoinstall", "web:local-disk"]
        );
        assert_eq!(
            pxe.log.calls(),
            vec!["pxe:custom-autoinstall", "pxe:local-disk"]
        );
        assert_eq!(registry.boot_target_now(TEST_MAC), LOCAL_DISK);
    }

    #[tokio::test]
    async fn test_flip_back_failure_surfaced() {
        let fleet = FleetConfig::default();
        let registry = MockRegistry::new(TEST_MAC, approved_machine());
        let web = MockWeb::new(CallLog::default());
        // Forward pxe write to custom-autoinstall succeeds; the fail-safe
        // flip-back call to local-disk fails.
        let pxe = MockPxe::failing_on(CallLog::default(), LOCAL_DISK);
        let power = MockPower::new(CallLog::default());
        let watch = MockWatch::always(CallLog::default(), Some(InstallStatus::Failed));
        let clock = TickClock::new(SystemTime::now());

        let deps = ReinstallDeps {
            web: &web,
            pxe: &pxe,
            power: &power,
            watch: &watch,
            registry: &registry,
            fleet: &fleet,
            clock: &clock,
            config: small_config(),
        };

        let outcome = reinstall_machine(
            &deps,
            ReinstallRequest {
                mac: TEST_MAC.to_string(),
                confirm: false,
            },
        )
        .await
        .unwrap();

        assert_eq!(
            outcome,
            ReinstallOutcome::TimedOutFlippedBack {
                flip_back_ok: false
            }
        );
        // The failed flip-back attempt was still made (and recorded) — never
        // swallowed.
        assert_eq!(
            pxe.log.calls(),
            vec!["pxe:custom-autoinstall", "pxe:local-disk"]
        );
    }

    #[tokio::test]
    async fn test_approved_host_reinstall_happy_path() {
        let fleet = FleetConfig::default();
        let shared = CallLog::default();
        let registry =
            MockRegistry::with_shared_write_log(TEST_MAC, approved_machine(), shared.clone());
        let web = MockWeb::new(shared.clone());
        let pxe = MockPxe::new(shared.clone());
        let power = MockPower::new(shared.clone());
        let watch = MockWatch::always(shared.clone(), Some(InstallStatus::Success));
        let clock = TickClock::new(SystemTime::now());

        let deps = ReinstallDeps {
            web: &web,
            pxe: &pxe,
            power: &power,
            watch: &watch,
            registry: &registry,
            fleet: &fleet,
            clock: &clock,
            config: small_config(),
        };

        let outcome = reinstall_machine(
            &deps,
            ReinstallRequest {
                mac: TEST_MAC.to_string(),
                confirm: false,
            },
        )
        .await
        .unwrap();

        assert_eq!(outcome, ReinstallOutcome::Done);

        let calls = shared.calls();
        assert_eq!(
            calls,
            vec![
                "registry:set_boot_target:custom-autoinstall".to_string(),
                "web:custom-autoinstall".to_string(),
                "pxe:custom-autoinstall".to_string(),
                format!("power:off:{TEST_HOST}"),
                format!("power:on:{TEST_HOST}"),
                "registry:stamp_reinstall".to_string(),
                format!("watch:{TEST_MAC}"),
            ]
        );
    }
}
