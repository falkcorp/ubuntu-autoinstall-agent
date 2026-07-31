// file: crates/uaa-core/src/network/ssh_installer/config.rs
// version: 2.15.0
// guid: sshcfg01-2345-6789-abcd-ef0123456789
// last-edited: 2026-07-27

//! Configuration structures for SSH/local installation

use crate::network::ssh_installer::components::firmware_quirks::FirmwareQuirk;
use crate::network::ssh_installer::components::hooks::Hooks;
use serde::{Deserialize, Serialize};

/// Which initramfs generator is in use on the target.
///
/// Dracut is used on the actual servers (Lenovo M715q) and requires different
/// regeneration commands + GRUB kernel parameters for Tang network unlock.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum InitramfsType {
    /// dracut — used on the Lenovo servers. Enables rd.neednet + Tang unlock at boot.
    #[default]
    Dracut,
    /// initramfs-tools — Ubuntu default (cloud images, live ISOs).
    InitramfsTools,
}

impl InitramfsType {
    /// Shell command to regenerate the initramfs inside a chroot at `/mnt/targetos`.
    pub fn regenerate_cmd(&self) -> &'static str {
        match self {
            Self::Dracut => "dracut --regenerate-all --force",
            Self::InitramfsTools => "update-initramfs -u -k all",
        }
    }
}

/// Classifies the role of a host: installation target or external Tang server.
///
/// This distinguishes between machines that will receive the autoinstall
/// (InstallTarget) and external Tang servers that provide network encryption
/// unlock. Note: HostRole::TangServer is the enum variant, distinct from the
/// TangServer struct below that describes a Tang server's connection details.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum HostRole {
    /// The default: this host is an installation target.
    #[default]
    InstallTarget,
    /// This host is an external Tang server providing Clevis unlock.
    TangServer,
}

impl HostRole {
    /// Returns true if this role is the installation target (the default).
    pub fn is_install_target(&self) -> bool {
        matches!(self, HostRole::InstallTarget)
    }
}

/// Tang server entry for Clevis SSS binding.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TangServer {
    pub url: String,
}

/// A workload assignable to a host. Closed-but-growing by design (spec
/// Decision 15): adding HAProxy/Keepalived later is a new variant, not a
/// plugin framework. An unknown `kind` is a hard parse error — never a
/// silent skip, because a silently-dropped application deploys a machine
/// missing its workload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum ApplicationSpec {
    Cockroach(CockroachSpec),
    TangServer(TangServerSpec),
    CockroachRolloutAgent(CockroachRolloutAgentSpec),
    PrometheusNodeExporter(PrometheusNodeExporterSpec),
    CanonicalLivepatch(CanonicalLivepatchSpec),
    ReportStatus(ReportStatusSpec),
    Zsh(ZshSpec),
}

impl ApplicationSpec {
    /// The wire `kind` tag for this variant.
    ///
    /// SINGLE SOURCE OF TRUTH. These strings previously lived in three
    /// unrelated places — `applications.rs`'s `reject_duplicates`,
    /// `profile/merge.rs`'s `app_kind`, and `scripts/vm-validate.sh` — with
    /// nothing tying them together, so adding a variant meant remembering
    /// three edits and a missed one silently degraded duplicate-rejection or
    /// merge-by-kind. The match below is exhaustive, so adding a variant to
    /// the enum without adding its tag here is now a COMPILE error.
    ///
    /// Must stay byte-identical to serde's `rename_all = "kebab-case"`
    /// rendering of the variant name — `config.rs`'s `kind_tags_match_serde`
    /// test asserts that against actual serialization.
    pub const fn kind(&self) -> &'static str {
        match self {
            ApplicationSpec::Cockroach(_) => "cockroach",
            ApplicationSpec::TangServer(_) => "tang-server",
            ApplicationSpec::CockroachRolloutAgent(_) => "cockroach-rollout-agent",
            ApplicationSpec::PrometheusNodeExporter(_) => "prometheus-node-exporter",
            ApplicationSpec::CanonicalLivepatch(_) => "canonical-livepatch",
            ApplicationSpec::ReportStatus(_) => "report-status",
            ApplicationSpec::Zsh(_) => "zsh",
        }
    }

    /// This application's drain policy, if it declares one.
    ///
    /// Lets U0 ask "does any application on this host need draining?" instead
    /// of "is this host one I remember runs a database?". A new clustered
    /// workload becomes safe to reinstall by authoring a `decommission` block
    /// on its variant — the reinstall driver never changes.
    ///
    /// `None` for stateless applications, which is most of them: a
    /// node-exporter or a login shell has nothing to drain.
    pub fn decommission(&self) -> Option<&DecommissionPolicy> {
        match self {
            ApplicationSpec::Cockroach(s) => Some(&s.decommission),
            ApplicationSpec::TangServer(_)
            | ApplicationSpec::CockroachRolloutAgent(_)
            | ApplicationSpec::PrometheusNodeExporter(_)
            | ApplicationSpec::CanonicalLivepatch(_)
            | ApplicationSpec::ReportStatus(_)
            | ApplicationSpec::Zsh(_) => None,
        }
    }
}

/// Whether any application on this host must be drained before its disk is
/// wiped. This is the whole of `NodeDrainer::needs_drain`'s logic — the
/// decision lives in the authored specs, not in the reinstall driver.
pub fn requires_drain(applications: &[ApplicationSpec]) -> bool {
    applications
        .iter()
        .filter_map(|a| a.decommission())
        .any(|d| d.enabled)
}

/// Every drain step declared across a host's applications, in application
/// order then step order.
pub fn drain_steps(applications: &[ApplicationSpec]) -> Vec<&DecommissionStep> {
    applications
        .iter()
        .filter_map(|a| a.decommission())
        .filter(|d| d.enabled)
        .flat_map(|d| d.steps.iter())
        .collect()
}

/// CockroachDB node parameters. `advertise`/`join` are NOT here: they are
/// DERIVED per host from the group's sibling list (profiles/TASK-04), never
/// authored. Defaults are the live fleet's values (verified 2026-07-16).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CockroachSpec {
    #[serde(default = "default_cockroach_version")]
    pub version: String,
    #[serde(default = "default_cockroach_port")]
    pub port: u16,
    #[serde(default = "default_cockroach_sql_port")]
    pub sql_port: u16,
    #[serde(default = "default_cockroach_http_addr")]
    pub http_addr: String,
    /// Cluster seed, always first in the join string.
    pub seed_ip: String,
    #[serde(default = "default_cockroach_cache")]
    pub cache: String,
    #[serde(default = "default_cockroach_max_sql")]
    pub max_sql_memory: String,
    #[serde(default = "default_cockroach_locality")]
    pub locality: String,
    /// `--store` value, verbatim.
    ///
    /// Default is the **len-serv-001/002** form. The installer previously
    /// hardcoded `/var/lib/cockroach/data`, which is len-serv-003's DRIFTED
    /// form — redeploying 003 would have faithfully recreated the outlier the
    /// pre-wipe inventory says to standardize away from.
    #[serde(default = "default_cockroach_store")]
    pub store: String,
    /// How U0 drains this node before a reinstall wipes it.
    ///
    /// Deliberately NOT `skip_serializing_if` — a policy governing a
    /// destructive, terminal operation should be visible in the placed
    /// artifact rather than implied by its absence.
    #[serde(default = "DecommissionPolicy::cockroach_default")]
    pub decommission: DecommissionPolicy,
}

/// A single step U0 runs to drain a host before its disk is wiped.
///
/// Closed enum, NOT free-form shell. These steps execute on **U0**, the fleet
/// control plane — unlike [`HookStep`](super::components::hooks::HookStep),
/// which runs on the target being installed and whose blast radius is a
/// machine already headed for a wipe. Making a registry profile blob able to
/// run arbitrary commands on U0 is a far larger promise, and a closed enum is
/// what every other component here uses (Decision 15: closed-but-growing
/// enums, not a plugin framework). Add a variant to support a new workload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "step", rename_all = "kebab-case", deny_unknown_fields)]
pub enum DecommissionStep {
    /// Stop a systemd unit on the host before draining, so it stops taking new
    /// work while its replicas move.
    StopUnit { unit: String },
    /// `cockroach node decommission` for this host's node id, using U0's
    /// certs. TERMINAL — the node can never rejoin under that id.
    CockroachDecommission,
    /// Poll the node's replica count until it reaches zero. This is the step
    /// that actually makes the wipe safe; `CockroachDecommission` only starts
    /// the drain.
    WaitForZeroReplicas,
}

/// Whether and how a host must be drained before a reinstall wipes it.
///
/// Declared BY THE APPLICATION rather than hardcoded in U0, so
/// `NodeDrainer::needs_drain` is "does any application declare
/// `decommission.enabled`?" instead of "is the hostname one we remember runs a
/// database?". A new clustered workload gets safe reinstalls by authoring this
/// block, with no change to the reinstall driver.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct DecommissionPolicy {
    /// `false` means a reinstall goes straight to the power cycle for this
    /// application — correct for anything stateless.
    #[serde(default)]
    pub enabled: bool,
    /// Ordered steps U0 runs. Empty with `enabled: true` is a config error
    /// (caught in validation), not a silent no-op — a host that claims it
    /// needs draining and specifies no way to drain is the worst case.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub steps: Vec<DecommissionStep>,
    /// Hard deadline for the whole drain. On expiry the reinstall is REFUSED,
    /// never forced — see `RefusalReason::DrainIncomplete`.
    #[serde(default = "default_decommission_timeout_secs")]
    pub timeout_secs: u64,
    /// Delay between replica-count polls.
    #[serde(default = "default_decommission_poll_secs")]
    pub poll_interval_secs: u64,
}

impl Default for DecommissionPolicy {
    fn default() -> Self {
        Self {
            enabled: false,
            steps: Vec::new(),
            timeout_secs: default_decommission_timeout_secs(),
            poll_interval_secs: default_decommission_poll_secs(),
        }
    }
}

impl DecommissionPolicy {
    /// The `skip_serializing_if` predicate: a stateless application omits the
    /// key entirely rather than serializing an all-defaults block.
    pub fn is_disabled_default(&self) -> bool {
        *self == Self::default()
    }

    /// The policy a CockroachDB node gets unless overridden: stop the unit so
    /// it takes no new work, decommission, then wait for replicas to hit zero.
    pub fn cockroach_default() -> Self {
        Self {
            enabled: true,
            steps: vec![
                DecommissionStep::StopUnit {
                    unit: "cockroach.service".to_string(),
                },
                DecommissionStep::CockroachDecommission,
                DecommissionStep::WaitForZeroReplicas,
            ],
            timeout_secs: default_decommission_timeout_secs(),
            poll_interval_secs: default_decommission_poll_secs(),
        }
    }
}

/// CockroachDB rollout agent. Binary + env file + unit, running as the
/// `cockroach` user alongside `cockroach.service`.
///
/// The unit shape mirrors the one deployed on len-serv-001/002 (read
/// 2026-07-30): `After=network-online.target cockroach.service`,
/// `EnvironmentFile=-` (leading `-` so a missing file is not fatal),
/// `ProtectSystem=strict` plus explicit `ReadWritePaths`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct CockroachRolloutAgentSpec {
    /// Where to fetch the agent binary. Required — there is no sane default
    /// and a wrong guess installs the wrong binary silently.
    pub binary_url: String,
    #[serde(default = "default_rollout_binary_path")]
    pub binary_path: String,
    /// CockroachDB connection string for the agent.
    ///
    /// SECRET-BEARING: authored as the `REPLACE_AT_PLACE_TIME` placeholder and
    /// substituted at place time, never committed. The live env file on
    /// len-serv-002 is explicitly marked "Host-local; do not commit".
    #[serde(default = "default_placeholder")]
    pub database_url: String,
    #[serde(default = "default_rollout_certs_dir")]
    pub certs_dir: String,
    #[serde(default = "default_rollout_artifacts_dir")]
    pub artifacts_dir: String,
    #[serde(default = "default_rollout_audit_log")]
    pub audit_log: String,
    /// Unit the agent restarts during a rollout.
    #[serde(default = "default_rollout_service")]
    pub service: String,
    /// Enable the unit at install time. len-serv-003 had it installed but
    /// DISABLED as of 2026-07-28, so this defaults to false to reproduce the
    /// fleet's actual state rather than an aspirational one.
    #[serde(default)]
    pub enabled: bool,
}

/// Prometheus node exporter. Distro package (`prometheus-node-exporter`),
/// so no version field — apt owns that.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct PrometheusNodeExporterSpec {
    /// `--web.listen-address`. Empty string keeps the packaged default.
    #[serde(default = "default_node_exporter_listen")]
    pub listen_address: String,
}

/// Canonical Livepatch. Installed as a snap, then enabled with a token.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct CanonicalLivepatchSpec {
    /// Livepatch token.
    ///
    /// SECRET-BEARING: authored as `REPLACE_AT_PLACE_TIME`, substituted at
    /// place time. Never commit a real token.
    #[serde(default = "default_placeholder")]
    pub key: String,
}

/// The `/usr/local/bin/report-status.sh` webhook reporter.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct ReportStatusSpec {
    /// Webhook endpoint. Default matches the deployed script on
    /// len-serv-002 (read 2026-07-30), where it was hardcoded.
    #[serde(default = "default_report_status_webhook")]
    pub webhook_url: String,
}

/// zsh as an operator's login shell, optionally with oh-my-zsh.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct ZshSpec {
    /// Account whose login shell becomes zsh.
    pub user: String,
    /// Install oh-my-zsh into that user's home. Requires network at install
    /// time (the upstream installer is fetched over HTTPS).
    #[serde(default = "default_true")]
    pub oh_my_zsh: bool,
}

/// Tang server workload parameters. Expressibility-only for now (rpi
/// hosts): the installer dispatch skips it with a warning rather than
/// applying it — no `tang-server` applier exists yet.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct TangServerSpec {
    /// Fleet Tang binds port 80 (verified 2026-07-16).
    #[serde(default = "default_tang_port")]
    pub port: u16,
    /// Directory holding the Tang server's advertised keys, e.g.
    /// `/etc/tang/keys`. Required — Tang has no sane default location.
    pub key_directory: String,
}

fn default_user_groups() -> Vec<String> {
    // Mirrors the len-serv cloud-init operator account (adm=read logs,
    // sudo=escalate, plus the usual desktop/container groups). Missing groups
    // are skipped at apply time via a `getent` guard, so listing docker/lxd on
    // a host that lacks them is harmless.
    vec![
        "adm".to_string(),
        "sudo".to_string(),
        "cdrom".to_string(),
        "dip".to_string(),
        "lxd".to_string(),
        "docker".to_string(),
    ]
}

fn default_user_shell() -> String {
    // bash is guaranteed present on a freshly-debootstrapped target; zsh (what
    // len-serv uses) would have to be installed first, so it is not the default.
    "/bin/bash".to_string()
}

/// A human operator account to provision on the installed target.
///
/// Created in the chroot with a home directory, added to `groups`, given
/// `password` via `chpasswd`, and seeded with its own `ssh_authorized_keys`.
/// This is the SSH/native-install analogue of the `users:` block the len-serv
/// cloud-init path already provisions, so both install paths yield the same
/// operator account.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct UserAccount {
    /// Login name, e.g. `jdfalk`.
    pub name: String,
    /// Plaintext password, applied via `chpasswd` (same handling as
    /// `root_password`). An empty string locks the password (`passwd -l`) so
    /// the account is SSH-key-only. NOTE: like `root_password`, a single quote
    /// in the value breaks the `echo '<name>:<pw>' | chpasswd` command — avoid
    /// `'` in passwords set through the installer.
    #[serde(default)]
    pub password: String,
    /// Supplementary groups. Default: adm, sudo, cdrom, dip, lxd, docker.
    /// Membership in `sudo` grants password-prompted `sudo`; no NOPASSWD
    /// sudoers entry is written (that is a deliberate choice for this path).
    #[serde(default = "default_user_groups")]
    pub groups: Vec<String>,
    /// Login shell. Default `/bin/bash`.
    #[serde(default = "default_user_shell")]
    pub shell: String,
    /// SSH public keys installed to `~/.ssh/authorized_keys` for this user.
    #[serde(default)]
    pub ssh_authorized_keys: Vec<String>,
}

/// Complete configuration for a machine installation.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstallationConfig {
    pub hostname: String,
    pub disk_device: String,
    pub timezone: String,
    pub luks_key: String,
    pub root_password: String,
    pub network_interface: String,
    pub network_address: String,
    pub network_gateway: String,
    pub network_search: String,
    pub network_nameservers: Vec<String>,
    /// Netplan renderer for the installed system: "networkd" (default) or
    /// "NetworkManager". Validated at render time.
    #[serde(default = "default_network_renderer")]
    pub network_renderer: String,
    pub debootstrap_release: Option<String>,
    pub debootstrap_mirror: Option<String>,
    /// Initramfs generator — defaults to Dracut.
    #[serde(default)]
    pub initramfs_type: InitramfsType,
    /// Tang servers for Clevis SSS binding. Empty = no Tang enrollment.
    #[serde(default)]
    pub tang_servers: Vec<TangServer>,
    /// SSS threshold (how many Tang servers must respond). Default 2.
    #[serde(default = "default_tang_threshold")]
    pub tang_threshold: u8,
    /// SSH public keys to install for root.
    #[serde(default)]
    pub ssh_authorized_keys: Vec<String>,
    /// Human operator accounts to provision on the target. Empty (the default,
    /// and the shape of every pre-existing config) provisions no login user —
    /// root + `ssh_authorized_keys` only, exactly as before.
    ///
    /// `skip_serializing_if` omits the key entirely for a user-free host, so a
    /// serialized (registry-resolved) config stays byte-identical to today's
    /// and parses on an older `uaa` binary whose `InstallationConfig`
    /// (`deny_unknown_fields`) predates this field — the same forward-compat
    /// contract `applications` relies on.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub users: Vec<UserAccount>,
    /// Enroll a TPM2 + PIN LUKS keyslot on first boot of the installed target.
    ///
    /// TPM2 must bind to the *installed* system's PCR values (not the live
    /// installer's), so enrollment happens via a oneshot systemd unit on first
    /// boot rather than during the unattended install. clevis's tpm2 pin has no
    /// PIN support, so this uses `systemd-cryptenroll --tpm2-with-pin` (unlocked
    /// at boot by the sd-cryptsetup dracut module, alongside clevis for Tang).
    #[serde(default = "default_true")]
    pub enroll_tpm2: bool,
    /// PIN required at boot for the TPM2 keyslot. Empty/None disables TPM2+PIN
    /// enrollment even when `enroll_tpm2` is true (no PIN = no anti-theft value).
    #[serde(default)]
    pub tpm2_pin: Option<String>,
    /// PCR indices the TPM2 policy binds to (comma-separated). Default "7"
    /// (secure-boot state). Kept minimal so routine kernel updates don't
    /// invalidate the binding; the PIN is the real anti-theft factor.
    #[serde(default = "default_tpm2_pcr_ids")]
    pub tpm2_pcr_ids: String,
    /// FIDO2 (YubiKey) unlock is enrolled MANUALLY post-install via
    /// `register-fido2-luks.sh` (needs the physical key + touch), so it is not
    /// part of the unattended install config. This flag only records intent /
    /// drives `verify` to check that at least one fido2 keyslot exists.
    #[serde(default = "default_true")]
    pub expect_fido2: bool,
    /// Install CA public cert (PEM), written to `/etc/uaa/install-ca.crt` on
    /// the target in Phase 5 so `uaa enroll`'s default `--ca` path finds it
    /// (spec Decision 7). NOT a per-host secret — the same cert for every
    /// host — so `uaa config place` fills this slot unconditionally from the
    /// server's `/var/lib/uaa/ca/ca.crt`, regardless of `--inject-from`. A
    /// config placed before the CA existed keeps the literal
    /// `REPLACE_AT_PLACE_TIME` placeholder here; Phase 5 writes it to the
    /// target as-is (fail-closed — `uaa enroll` treats an unparseable CA as
    /// the missing-CA case, never falling back to system roots).
    #[serde(default = "default_install_ca_cert")]
    pub install_ca_cert: String,
    /// Applications to install into the target during Phase 5. Empty = none,
    /// which is exactly today's behavior for every committed host config.
    ///
    /// `skip_serializing_if` omits the key entirely for an app-free host so a
    /// serialized (registry-resolved) config is byte-safe across a control
    /// rollback: a placed config that never gained an `applications:` key can
    /// still be parsed by an older `uaa install` binary whose
    /// `InstallationConfig` (deny_unknown_fields) predates the field. Without
    /// this, an app-free host would serialize `applications: []` and trip a
    /// fail-closed parse on every PXE after a rollback (DS-OPS-03).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub applications: Vec<ApplicationSpec>,
    /// The Cockroach group roster for THIS host's cluster: sibling member
    /// IPs (bare, no CIDR) that `applications.rs::derive_cockroach_endpoints`
    /// consumes to build the advertise/join strings. Populated by
    /// `uaa-control`'s `resolve_from_registry` from the active group
    /// allocation (PS-COCKROACH-16) — never hand-authored. Empty for every
    /// host without a Cockroach application (today's entire committed fleet).
    ///
    /// `skip_serializing_if` omits the key for a Cockroach-free host, so a
    /// registry-resolved config serializes exactly as before this field
    /// existed — same cross-version-rollback rationale as `applications`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cockroach_members: Vec<String>,
    /// Storage layout the installer builds for this host. Defaults to
    /// [`StorageMode::PlainLuks`] — the single-disk ZFS-on-LUKS path every
    /// Lenovo (`len-serv-*`) uses today — so a config that omits the key is
    /// byte-for-byte unchanged. Only `unimatrixone` sets `NativeKeystore`.
    /// See `docs/specs/u1-zfs-native-encryption-{design,plan}.md`.
    ///
    /// `skip_serializing_if` omits the key for a `PlainLuks` host, so a
    /// registry-resolved Lenovo config serializes exactly as before — no new
    /// `storage-mode:` key to trip an older `deny_unknown_fields` binary on a
    /// control rollback (same rationale as `applications`).
    #[serde(default, skip_serializing_if = "StorageMode::is_default")]
    pub storage_mode: StorageMode,
    /// Multi-disk roster for [`StorageMode::NativeKeystore`] (by-id device
    /// paths + roles). Ignored under `PlainLuks`, which uses `disk_device`.
    /// `skip_serializing_if` keeps a `PlainLuks` config's serialization free of
    /// an empty `disks: []` key (same cross-version-rollback safety as
    /// `applications`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub disks: Vec<DiskSpec>,
    /// Target architecture (amd64/arm64). Defaults to `Amd64` — the entire
    /// fleet today — so an omitting host serializes byte-identically.
    /// `skip_serializing_if` keeps the key out of a stock amd64 config's
    /// serialization (same cross-version-rollback rationale as
    /// `storage_mode`). No behavior consumes this yet.
    #[serde(default, skip_serializing_if = "Arch::is_amd64")]
    pub arch: Arch,
    /// Installation-target vs. external-Tang-server classifier. Defaults to
    /// `InstallTarget` — every committed host config today — so an omitting
    /// host serializes byte-identically. No behavior consumes this yet.
    #[serde(default, skip_serializing_if = "HostRole::is_install_target")]
    pub role: HostRole,
    /// Per-board firmware/boot-loader workarounds (PS-QUIRK-05). Empty = none,
    /// today's behavior for every committed host. No behavior consumes this
    /// yet.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub firmware_quirks: Vec<FirmwareQuirk>,
    /// Arbitrary host-specific commands at named install phases (PS-HOOK-06).
    /// Empty = none, today's behavior for every committed host. No behavior
    /// consumes this yet.
    #[serde(default, skip_serializing_if = "Hooks::is_empty")]
    pub hooks: Hooks,
}

/// Which encryption/storage layout the installer builds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum StorageMode {
    /// Single disk → LUKS → ZFS `rpool` on the mapper. The proven Lenovo path
    /// (`disk_ops::prepare_disk`); `disks` is ignored, `disk_device` drives it.
    #[default]
    PlainLuks,
    /// ZFS **native** encryption on the Ubuntu keystore-zvol layout across the
    /// multi-disk `disks` roster: bulk data mirror + Optane `special` metadata
    /// mirror, a `rpool/keystore` zvol, clevis SSS unlock. U1 only.
    NativeKeystore,
}

impl StorageMode {
    /// `true` for the default (`PlainLuks`) — the serde `skip_serializing_if`
    /// predicate that keeps a Lenovo config's serialization key-for-key unchanged.
    pub fn is_default(&self) -> bool {
        matches!(self, StorageMode::PlainLuks)
    }
}

/// Target architecture classifier — independent of `crate::config::Architecture`.
/// This one belongs to the SSH installer pipeline; the retired `Architecture`
/// enum belongs to the legacy TargetConfig/image pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum Arch {
    /// x86-64 / AMD64 — default and most common in the fleet.
    #[default]
    Amd64,
    /// ARM 64-bit — used by some lightweight servers (e.g., RPi).
    Arm64,
}

impl Arch {
    /// `true` for the default (`Amd64`) — intended for use in a future
    /// `#[serde(skip_serializing_if="Arch::is_amd64")]` to omit the field
    /// for AMD64 hosts (mirroring the `StorageMode::is_default()` precedent).
    pub fn is_amd64(&self) -> bool {
        matches!(self, Arch::Amd64)
    }
}

/// Role a physical disk plays in the [`StorageMode::NativeKeystore`] layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DiskRole {
    /// Bootable bulk device (SATA SSD): carries the ESP + bpool member + the
    /// `rpool` **data** vdev member (as `p3`). This is the boot disk — the
    /// X10DSC+ firmware cannot boot from NVMe, so the ESP/bootloader must live
    /// here, not on the Optane (see [`super::layout`], design 2026-07-23).
    System,
    /// Fast small device (Optane): a **half-disk** `rpool` `special` (metadata)
    /// vdev member. The other half is left free, reserved for a future
    /// spinning-disk array's special vdev.
    Special,
}

/// One disk in the [`StorageMode::NativeKeystore`] roster.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiskSpec {
    /// Stable device path — `/dev/disk/by-id/...`, **never** `sdX`/`nvmeXnY`
    /// (enumeration order is not stable across boots on a 4-drive box).
    pub id: String,
    /// What this disk is for.
    pub role: DiskRole,
}

fn default_tang_threshold() -> u8 {
    2
}

fn default_true() -> bool {
    true
}

fn default_tpm2_pcr_ids() -> String {
    "7".to_string()
}

pub fn default_install_ca_cert() -> String {
    crate::config_place::PLACEHOLDER.to_string()
}

pub fn default_network_renderer() -> String {
    "networkd".to_string()
}

fn default_cockroach_version() -> String {
    "v25.3.0".to_string()
}

fn default_cockroach_port() -> u16 {
    36357
}

fn default_cockroach_sql_port() -> u16 {
    36257
}

fn default_cockroach_http_addr() -> String {
    ":38080".to_string()
}

fn default_cockroach_cache() -> String {
    ".25".to_string()
}

fn default_cockroach_max_sql() -> String {
    ".25".to_string()
}

fn default_cockroach_locality() -> String {
    "region=us,cluster-unit=lenovo".to_string()
}

fn default_tang_port() -> u16 {
    80
}

/// One hour. A drain moving hundreds of replicas off a node is not fast, and
/// the failure mode of a too-short deadline is a REFUSED reinstall, not a
/// forced one — so err long.
fn default_decommission_timeout_secs() -> u64 {
    3600
}

fn default_decommission_poll_secs() -> u64 {
    30
}

/// len-serv-001/002 form, verified 2026-07-30. NOT len-serv-003's drifted
/// bare `/var/lib/cockroach/data`.
fn default_cockroach_store() -> String {
    "path=/var/lib/cockroach/cockroach-data,attrs=ssd,size=.5".to_string()
}

/// Secret-bearing fields author as this and are substituted at place time.
fn default_placeholder() -> String {
    "REPLACE_AT_PLACE_TIME".to_string()
}

fn default_rollout_binary_path() -> String {
    "/usr/local/bin/cockroach-rollout-agent".to_string()
}

fn default_rollout_certs_dir() -> String {
    "/var/lib/cockroach/certs".to_string()
}

fn default_rollout_artifacts_dir() -> String {
    "/var/lib/cockroach-rollout-agent".to_string()
}

fn default_rollout_audit_log() -> String {
    "/var/log/cockroach-rollout-agent/audit.log".to_string()
}

fn default_rollout_service() -> String {
    "cockroach.service".to_string()
}

fn default_node_exporter_listen() -> String {
    ":9100".to_string()
}

/// Matches the hardcoded URL in the deployed `/usr/local/bin/report-status.sh`
/// on len-serv-002 (read 2026-07-30).
fn default_report_status_webhook() -> String {
    "http://172.16.2.30:25000/api/webhook".to_string()
}

impl InstallationConfig {
    /// Load configuration from a YAML file.
    pub fn from_yaml_file(path: &str) -> crate::Result<Self> {
        let content =
            std::fs::read_to_string(path).map_err(crate::error::AutoInstallError::IoError)?;
        serde_yaml::from_str(&content).map_err(crate::error::AutoInstallError::SerdeError)
    }

    /// Create the production config for len-serv-003 (172.16.3.96).
    pub fn for_len_serv_003() -> Self {
        Self {
            hostname: "len-serv-003".to_string(),
            disk_device: "/dev/nvme0n1".to_string(),
            timezone: "America/New_York".to_string(),
            luks_key: "changeme123!@#".to_string(),
            root_password: "changeme123!@#".to_string(),
            network_interface: "enp1s0f0".to_string(),
            network_address: "172.16.3.96/23".to_string(),
            network_gateway: "172.16.2.1".to_string(),
            network_search: "jf.local".to_string(),
            network_nameservers: vec![
                "172.16.2.1".to_string(),
                "1.1.1.1".to_string(),
                "8.8.8.8".to_string(),
            ],
            network_renderer: default_network_renderer(),
            debootstrap_release: Some("resolute".to_string()),
            debootstrap_mirror: Some("http://archive.ubuntu.com/ubuntu/".to_string()),
            initramfs_type: InitramfsType::Dracut,
            tang_servers: vec![
                TangServer {
                    url: "http://172.16.2.45".to_string(),
                },
                TangServer {
                    url: "http://172.16.2.46".to_string(),
                },
                TangServer {
                    url: "http://172.16.2.47".to_string(),
                },
            ],
            tang_threshold: 2,
            ssh_authorized_keys: vec![
                "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOq0x6/0fA+vn0EdNJvBuadOo4rZ1IwkCWbBOWCwvId5 jdfalk@Norn.lan".to_string(),
                "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIP4PPvBh1cCMdh8S5Uqz/1cONHxhc78TfWLt0fx76B/G jdfalk@JohnathsMacBook.jf.local".to_string(),
                "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIPghsb0DAzQX5LfLgb1Q11LJJhppTM1r093TWCTjxjdb eddsa-key-20220820".to_string(),
            ],
            // No operator user in the committed fallback — real accounts (with
            // passwords) live only in the uncommitted per-host YAML.
            users: Vec::new(),
            enroll_tpm2: true,
            // Placeholder — the real PIN is injected per-host from a secret at
            // seed-render time, never committed. None here disables TPM2 in the
            // hardcoded fallback config.
            tpm2_pin: None,
            tpm2_pcr_ids: default_tpm2_pcr_ids(),
            expect_fido2: true,
            install_ca_cert: default_install_ca_cert(),
            applications: Vec::new(),
            cockroach_members: Vec::new(),
            storage_mode: StorageMode::PlainLuks,
            disks: Vec::new(),
            arch: Arch::Amd64,
            role: HostRole::InstallTarget,
            firmware_quirks: Vec::new(),
            hooks: Hooks::default(),
        }
    }
}

/// Collected information about the target system.
#[derive(Debug, Default)]
pub struct SystemInfo {
    pub hostname: String,
    pub kernel_version: String,
    pub os_release: String,
    pub disk_info: String,
    pub network_info: String,
    pub available_tools: Vec<String>,
    pub memory_info: String,
    pub cpu_info: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_initramfs_type_default_is_dracut() {
        assert_eq!(InitramfsType::default(), InitramfsType::Dracut);
    }

    /// [`ApplicationSpec::kind`] must return exactly what serde writes as the
    /// `kind` tag. The two are independent code paths — a hand-written match
    /// versus `rename_all = "kebab-case"` — so nothing but this test stops
    /// them drifting. Drift is silent and nasty: `reject_duplicates` and
    /// merge-by-kind would key on a string that never appears on the wire, so
    /// duplicate detection and host-over-group replacement would both quietly
    /// stop working for that variant.
    #[test]
    fn kind_tags_match_serde() {
        fn serialized_kind(spec: &ApplicationSpec) -> String {
            let v = serde_json::to_value(spec).expect("serialize");
            v.get("kind")
                .and_then(|k| k.as_str())
                .expect("tagged enum always emits a `kind` string")
                .to_string()
        }

        // One value per variant. Adding a variant without adding it here
        // leaves it unchecked, so the exhaustive match in `kind()` is the
        // real backstop; this pins the STRINGS.
        let samples = vec![
            ApplicationSpec::Cockroach(CockroachSpec {
                version: "v25.3.0".into(),
                port: 36357,
                sql_port: 36257,
                http_addr: ":38080".into(),
                seed_ip: "172.16.3.92".into(),
                cache: ".25".into(),
                max_sql_memory: ".25".into(),
                locality: "region=us".into(),
                store: default_cockroach_store(),
                decommission: DecommissionPolicy::cockroach_default(),
            }),
            ApplicationSpec::TangServer(TangServerSpec {
                port: 80,
                key_directory: "/etc/tang/keys".into(),
            }),
            ApplicationSpec::CockroachRolloutAgent(CockroachRolloutAgentSpec {
                binary_url: "http://example/agent".into(),
                binary_path: default_rollout_binary_path(),
                database_url: default_placeholder(),
                certs_dir: default_rollout_certs_dir(),
                artifacts_dir: default_rollout_artifacts_dir(),
                audit_log: default_rollout_audit_log(),
                service: default_rollout_service(),
                enabled: false,
            }),
            ApplicationSpec::PrometheusNodeExporter(PrometheusNodeExporterSpec {
                listen_address: default_node_exporter_listen(),
            }),
            ApplicationSpec::CanonicalLivepatch(CanonicalLivepatchSpec {
                key: default_placeholder(),
            }),
            ApplicationSpec::ReportStatus(ReportStatusSpec {
                webhook_url: default_report_status_webhook(),
            }),
            ApplicationSpec::Zsh(ZshSpec {
                user: "jdfalk".into(),
                oh_my_zsh: true,
            }),
        ];

        for spec in &samples {
            assert_eq!(
                spec.kind(),
                serialized_kind(spec),
                "ApplicationSpec::kind() disagrees with the serde `kind` tag"
            );
        }

        // Guard the count so a new variant added to the enum without a sample
        // above fails loudly rather than passing vacuously.
        assert_eq!(samples.len(), 7, "add the new ApplicationSpec variant here");
    }

    /// `requires_drain` is the whole of U0's `needs_drain` decision, so it must
    /// key off the authored spec — never off a hostname or a variant name.
    #[test]
    fn requires_drain_is_declared_by_the_spec() {
        let stateless = vec![
            ApplicationSpec::PrometheusNodeExporter(PrometheusNodeExporterSpec {
                listen_address: default_node_exporter_listen(),
            }),
            ApplicationSpec::Zsh(ZshSpec {
                user: "jdfalk".into(),
                oh_my_zsh: false,
            }),
        ];
        assert!(
            !requires_drain(&stateless),
            "a host running nothing clustered must not be drained"
        );
        assert!(drain_steps(&stateless).is_empty());

        let mut crdb = CockroachSpec {
            version: "v25.3.0".into(),
            port: 36357,
            sql_port: 36257,
            http_addr: ":38080".into(),
            seed_ip: "172.16.3.92".into(),
            cache: ".25".into(),
            max_sql_memory: ".25".into(),
            locality: "region=us".into(),
            store: default_cockroach_store(),
            decommission: DecommissionPolicy::cockroach_default(),
        };

        let with_crdb = vec![ApplicationSpec::Cockroach(crdb.clone())];
        assert!(requires_drain(&with_crdb));
        assert_eq!(
            drain_steps(&with_crdb).len(),
            3,
            "stop unit, decommission, wait for zero replicas"
        );

        // Turning the policy off is honored — the decision is authored, not
        // inferred from the fact that it is a database.
        crdb.decommission.enabled = false;
        assert!(!requires_drain(&[ApplicationSpec::Cockroach(crdb)]));
    }

    /// A `--store` value must yield the directory cockroach will actually
    /// create, in both forms the flag accepts.
    #[test]
    fn store_directory_handles_both_forms() {
        use crate::network::ssh_installer::applications::store_directory;
        assert_eq!(
            store_directory("path=/var/lib/cockroach/cockroach-data,attrs=ssd,size=.5"),
            "/var/lib/cockroach/cockroach-data",
            "key=value form (len-serv-001/002)"
        );
        assert_eq!(
            store_directory("/var/lib/cockroach/data"),
            "/var/lib/cockroach/data",
            "bare-path form (len-serv-003's drift)"
        );
    }

    #[test]
    fn test_dracut_regenerate_cmd() {
        assert_eq!(
            InitramfsType::Dracut.regenerate_cmd(),
            "dracut --regenerate-all --force"
        );
    }

    #[test]
    fn test_initramfs_tools_regenerate_cmd() {
        assert_eq!(
            InitramfsType::InitramfsTools.regenerate_cmd(),
            "update-initramfs -u -k all"
        );
    }

    #[test]
    fn test_for_len_serv_003_has_tang_servers() {
        let cfg = InstallationConfig::for_len_serv_003();
        assert_eq!(cfg.tang_servers.len(), 3);
        assert_eq!(cfg.tang_threshold, 2);
        assert_eq!(cfg.initramfs_type, InitramfsType::Dracut);
    }

    #[test]
    fn test_for_len_serv_003_network() {
        let cfg = InstallationConfig::for_len_serv_003();
        assert_eq!(cfg.network_address, "172.16.3.96/23");
        assert_eq!(cfg.network_interface, "enp1s0f0");
    }

    #[test]
    fn test_for_len_serv_003_multikey_defaults() {
        let cfg = InstallationConfig::for_len_serv_003();
        // TPM2+PIN and FIDO2 expectations default on; the PIN itself is injected
        // per-host from a secret (None in the hardcoded fallback).
        assert!(cfg.enroll_tpm2);
        assert_eq!(cfg.tpm2_pin, None);
        assert_eq!(cfg.tpm2_pcr_ids, "7");
        assert!(cfg.expect_fido2);
    }

    #[test]
    fn test_install_example_configs_round_trip() {
        // The committed per-host example configs under examples/configs/install/
        // must deserialize into InstallationConfig with the multi-key features
        // explicitly enabled (they must NOT rely on serde defaults for tang/tpm2).
        // Scoped to these four files only — the legacy examples/configs/*.yaml use
        // an older, incompatible schema and are intentionally not loaded here.
        let load = |host: &str| -> InstallationConfig {
            let path = format!(
                "{}/../../examples/configs/install/{}.yaml",
                env!("CARGO_MANIFEST_DIR"),
                host
            );
            InstallationConfig::from_yaml_file(&path)
                .unwrap_or_else(|e| panic!("{host} config must parse: {e}"))
        };

        // The len-servs are the PlainLuks (legacy single-disk) path.
        let plain = [
            ("len-serv-001", "/dev/nvme0n1", "172.16.3.92/23"),
            ("len-serv-002", "/dev/nvme0n1", "172.16.3.94/23"),
            ("len-serv-003", "/dev/nvme0n1", "172.16.3.96/23"),
        ];
        for (host, disk, addr) in plain {
            let cfg = load(host);
            assert_eq!(cfg.storage_mode, StorageMode::PlainLuks, "{host}: PlainLuks");
            assert_eq!(cfg.hostname, host, "{host}: hostname");
            assert_eq!(cfg.disk_device, disk, "{host}: disk_device");
            assert_eq!(cfg.network_address, addr, "{host}: network_address");
            assert_eq!(cfg.initramfs_type, InitramfsType::Dracut, "{host}: dracut");
            assert_eq!(cfg.tang_servers.len(), 3, "{host}: 3 tang servers");
            assert_eq!(cfg.tang_threshold, 2, "{host}: tang threshold");
            assert!(cfg.enroll_tpm2, "{host}: enroll_tpm2");
            assert!(cfg.expect_fido2, "{host}: expect_fido2");
            assert_eq!(
                cfg.tpm2_pin.as_deref(),
                Some("REPLACE_AT_PLACE_TIME"),
                "{host}: tpm2_pin placeholder"
            );
            assert_eq!(cfg.luks_key, "REPLACE_AT_PLACE_TIME", "{host}: luks_key placeholder");
            assert_eq!(cfg.root_password, "REPLACE_AT_PLACE_TIME", "{host}: root_password");
        }

        // unimatrixone is the NativeKeystore (ZFS native-encryption) path — the
        // future server profile. Different unlock policy: enroll_tpm2/expect_fido2
        // OFF (D2-B uses a clevis tpm2 pin, not the hanging systemd-tpm2 token).
        // unimatrixone is now a SANITIZED NativeKeystore TEMPLATE: host-unique
        // values (disk serials, IP, MAC, hostname) are REPLACE_AT_PLACE_TIME
        // placeholders — real values live in the registry backend, not git (no
        // fleet-topology/MAC exposure). We assert the NativeKeystore *shape* and
        // that the host-unique fields are placeholders, not real values.
        let u1 = load("unimatrixone");
        assert_eq!(u1.storage_mode, StorageMode::NativeKeystore, "u1: NativeKeystore");
        assert_eq!(u1.initramfs_type, InitramfsType::Dracut, "u1: dracut");
        // 4-disk roster shape: 2 system (bootable SATA SSD) + 2 special (Optane).
        assert_eq!(u1.disks.len(), 4, "u1: 4-disk roster");
        assert_eq!(
            u1.disks.iter().filter(|d| d.role == DiskRole::System).count(),
            2,
            "u1: 2 system (SSD) disks"
        );
        assert_eq!(
            u1.disks.iter().filter(|d| d.role == DiskRole::Special).count(),
            2,
            "u1: 2 special (Optane) disks"
        );
        assert_eq!(u1.tang_servers.len(), 3, "u1: 3 tang servers");
        assert_eq!(u1.tang_threshold, 2, "u1: tang threshold (D2-B t=2)");
        assert!(!u1.enroll_tpm2, "u1: enroll_tpm2 OFF (clevis tpm2 pin instead)");
        assert!(!u1.expect_fido2, "u1: expect_fido2 OFF");
        // Host-unique fleet data (IP, disk serials) must be sanitized
        // placeholders — never real topology / spoofable identifiers committed
        // to the repo. (hostname is just a name, not sensitive; it's also what
        // the registry derives on resolve, so it stays real.)
        assert_eq!(u1.hostname, "unimatrixone", "u1: hostname");
        assert_eq!(u1.network_address, "REPLACE_AT_PLACE_TIME", "u1: address placeholder");
        assert_eq!(u1.luks_key, "REPLACE_AT_PLACE_TIME", "u1: luks_key placeholder");
        assert!(
            u1.disks.iter().all(|d| d.id.starts_with("REPLACE_AT_PLACE_TIME")),
            "u1: disk ids must be place-time placeholders, not real serials"
        );
    }

    #[test]
    fn test_multikey_serde_defaults_when_absent() {
        // A minimal YAML with none of the new fields must deserialize with the
        // secure defaults (TPM2 on, PCR 7, FIDO2 expected) rather than failing.
        let yaml = r#"
hostname: test
disk_device: /dev/sda
timezone: UTC
luks_key: k
root_password: p
network_interface: eth0
network_address: 10.0.0.2/24
network_gateway: 10.0.0.1
network_search: local
network_nameservers: ["10.0.0.1"]
"#;
        let cfg: InstallationConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(cfg.enroll_tpm2);
        assert_eq!(cfg.tpm2_pcr_ids, "7");
        assert!(cfg.expect_fido2);
        assert_eq!(cfg.tpm2_pin, None);
    }

    #[test]
    fn test_unknown_yaml_key_rejected() {
        // deny_unknown_fields: a typo'd key must fail parsing loudly, not be
        // silently dropped (this installer LUKS-formats disks off this config).
        let yaml = r#"
hostname: test
disk_devise: /dev/sda
disk_device: /dev/sda
timezone: UTC
luks_key: k
root_password: p
network_interface: eth0
network_address: 10.0.0.2/24
network_gateway: 10.0.0.1
network_search: local
network_nameservers: ["10.0.0.1"]
"#;
        let err = serde_yaml::from_str::<InstallationConfig>(yaml).unwrap_err();
        assert!(err.to_string().contains("disk_devise"), "error must name the unknown key: {err}");
    }

    #[test]
    fn test_network_renderer_defaults_when_absent() {
        // Old committed YAML has no `network_renderer` key; the serde default
        // must keep it parsing (and defaulting to "networkd") unchanged.
        let cfg = InstallationConfig::for_len_serv_003();
        let yaml = serde_yaml::to_string(&cfg).unwrap();
        let yaml_without_renderer: String = yaml
            .lines()
            .filter(|l| !l.contains("network_renderer"))
            .collect::<Vec<_>>()
            .join("\n");
        let back: InstallationConfig = serde_yaml::from_str(&yaml_without_renderer).unwrap();
        assert_eq!(back.network_renderer, "networkd");
    }

    #[test]
    fn test_applications_defaults_to_empty_when_absent() {
        // A minimal YAML with no `applications:` key must deserialize with an
        // empty applications list, not fail — this is every committed host
        // config today.
        let yaml = r#"
hostname: test
disk_device: /dev/sda
timezone: UTC
luks_key: k
root_password: p
network_interface: eth0
network_address: 10.0.0.2/24
network_gateway: 10.0.0.1
network_search: local
network_nameservers: ["10.0.0.1"]
"#;
        let cfg: InstallationConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(cfg.applications.is_empty());
    }

    #[test]
    fn test_applications_empty_is_todays_behavior() {
        assert!(InstallationConfig::for_len_serv_003().applications.is_empty());
    }

    #[test]
    fn test_app_free_host_omits_applications_key() {
        // Cross-version rollback safety (DS-OPS-03): a host with no
        // applications must serialize WITHOUT an `applications:` key at all, so
        // a rolled-back `uaa install` binary (whose deny_unknown_fields
        // InstallationConfig predates the field) still parses the placed file
        // instead of hitting a fail-closed parse on every PXE.
        let cfg = InstallationConfig::for_len_serv_003();
        assert!(cfg.applications.is_empty(), "fixture must be app-free");
        let yaml = serde_yaml::to_string(&cfg).unwrap();
        assert!(
            !yaml.contains("applications"),
            "an app-free host must omit the applications key entirely, got:\n{yaml}"
        );
    }

    #[test]
    fn test_cockroach_free_host_omits_cockroach_members_key() {
        // Same cross-version-rollback discipline as
        // test_app_free_host_omits_applications_key (DS-OPS-03): a host with
        // no Cockroach application (every committed host today) must
        // serialize WITHOUT a `cockroach_members:` key at all.
        let cfg = InstallationConfig::for_len_serv_003();
        assert!(cfg.cockroach_members.is_empty(), "fixture must be cockroach-free");
        let yaml = serde_yaml::to_string(&cfg).unwrap();
        assert!(
            !yaml.contains("cockroach_members"),
            "a cockroach-free host must omit the cockroach_members key entirely, got:\n{yaml}"
        );
    }

    #[test]
    fn test_cockroach_members_defaults_to_empty_when_absent() {
        let yaml = r#"
hostname: test
disk_device: /dev/sda
timezone: UTC
luks_key: k
root_password: p
network_interface: eth0
network_address: 10.0.0.2/24
network_gateway: 10.0.0.1
network_search: local
network_nameservers: ["10.0.0.1"]
"#;
        let cfg: InstallationConfig = serde_yaml::from_str(yaml).unwrap();
        assert!(cfg.cockroach_members.is_empty());
    }

    #[test]
    fn test_cockroach_members_round_trips_when_present() {
        let mut cfg = InstallationConfig::for_len_serv_003();
        cfg.cockroach_members = vec!["172.16.3.92".to_string(), "172.16.3.94".to_string()];
        let yaml = serde_yaml::to_string(&cfg).unwrap();
        assert!(yaml.contains("cockroach_members"), "non-empty cockroach_members must serialize, got:\n{yaml}");
        let back: InstallationConfig = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(back.cockroach_members, cfg.cockroach_members);
    }

    #[test]
    fn test_plain_luks_host_omits_storage_keys() {
        // Cross-version rollback safety (U1 Phase 1): a stock PlainLuks host —
        // every len-serv — must serialize WITHOUT `storage-mode:` or `disks:`,
        // so a rolled-back control binary (whose deny_unknown_fields
        // InstallationConfig predates the U1 keystore fields) still parses the
        // placed file byte-for-byte as it did before U1. Only unimatrixone,
        // which sets NativeKeystore, emits these keys.
        let cfg = InstallationConfig::for_len_serv_003();
        assert_eq!(cfg.storage_mode, StorageMode::PlainLuks, "fixture is PlainLuks");
        assert!(cfg.disks.is_empty(), "fixture has no multi-disk roster");
        let yaml = serde_yaml::to_string(&cfg).unwrap();
        assert!(
            !yaml.contains("storage-mode") && !yaml.contains("storage_mode"),
            "a PlainLuks host must omit the storage-mode key entirely, got:\n{yaml}"
        );
        assert!(
            !yaml.contains("disks"),
            "a PlainLuks host must omit the disks key entirely, got:\n{yaml}"
        );
    }

    #[test]
    fn test_native_keystore_host_emits_storage_mode() {
        // The inverse guard: when a host IS NativeKeystore the discriminator must
        // actually appear (the field key is snake_case like every other
        // InstallationConfig field; only the enum *value* is kebab-cased), else
        // the installer would silently fall back to the PlainLuks path on U1.
        let mut cfg = InstallationConfig::for_len_serv_003();
        cfg.storage_mode = StorageMode::NativeKeystore;
        cfg.disks = vec![DiskSpec {
            id: "/dev/disk/by-id/nvme-optane".to_string(),
            role: DiskRole::System,
        }];
        let yaml = serde_yaml::to_string(&cfg).unwrap();
        assert!(
            yaml.contains("storage_mode: native-keystore"),
            "NativeKeystore must serialize the discriminator (kebab-case value), got:\n{yaml}"
        );
        assert!(yaml.contains("disks"), "a NativeKeystore host must emit its disks roster");
    }

    #[test]
    fn test_cockroach_spec_defaults() {
        let yaml = r#"
kind: cockroach
seed_ip: 172.16.3.92
"#;
        let spec: ApplicationSpec = serde_yaml::from_str(yaml).unwrap();
        let ApplicationSpec::Cockroach(cockroach) = spec else {
            panic!("expected Cockroach variant, got {spec:?}");
        };
        assert_eq!(cockroach.version, "v25.3.0");
        assert_eq!(cockroach.port, 36357);
        assert_eq!(cockroach.sql_port, 36257);
        assert_eq!(cockroach.cache, ".25");
        assert_eq!(cockroach.locality, "region=us,cluster-unit=lenovo");
    }

    #[test]
    fn test_unknown_application_kind_rejected() {
        // The enum is closed by design (spec Decision 15): an unknown kind
        // must be a hard parse error naming the unknown kind, never a silent
        // skip.
        let yaml = r#"
kind: redis
"#;
        let err = serde_yaml::from_str::<ApplicationSpec>(yaml).unwrap_err();
        assert!(err.to_string().contains("redis"), "error must name the unknown kind: {err}");
    }

    #[test]
    fn test_cockroach_spec_unknown_field_rejected() {
        let yaml = r#"
kind: cockroach
seed_ip: 172.16.3.92
typo_field: oops
"#;
        let err = serde_yaml::from_str::<ApplicationSpec>(yaml).unwrap_err();
        assert!(
            err.to_string().contains("typo_field"),
            "error must name the unknown field: {err}"
        );
    }

    #[test]
    fn test_arch_default_is_amd64() {
        assert_eq!(Arch::default(), Arch::Amd64);
    }

    #[test]
    fn test_arch_is_amd64() {
        assert!(Arch::Amd64.is_amd64());
        assert!(!Arch::Arm64.is_amd64());
    }

    #[test]
    fn test_arch_serde_round_trip() {
        // Amd64 should serialize to "amd64"
        let amd64_yaml = serde_yaml::to_string(&Arch::Amd64).unwrap();
        assert_eq!(amd64_yaml.trim(), "amd64");

        // Arm64 should serialize to "arm64"
        let arm64_yaml = serde_yaml::to_string(&Arch::Arm64).unwrap();
        assert_eq!(arm64_yaml.trim(), "arm64");

        // Round-trip: deserialize back
        let amd64_back: Arch = serde_yaml::from_str("amd64").unwrap();
        assert_eq!(amd64_back, Arch::Amd64);

        let arm64_back: Arch = serde_yaml::from_str("arm64").unwrap();
        assert_eq!(arm64_back, Arch::Arm64);
    }

    #[test]
    fn test_host_role_default_is_install_target() {
        assert_eq!(HostRole::default(), HostRole::InstallTarget);
    }

    #[test]
    fn test_host_role_is_install_target() {
        assert!(HostRole::InstallTarget.is_install_target());
        assert!(!HostRole::TangServer.is_install_target());
    }

    #[test]
    fn test_host_role_serde_round_trip() {
        // Verify wire strings match kebab-case naming.
        let install_target_yaml = "install-target";
        let deserialized: HostRole = serde_yaml::from_str(install_target_yaml).unwrap();
        assert_eq!(deserialized, HostRole::InstallTarget);
        let serialized = serde_yaml::to_string(&deserialized).unwrap();
        assert!(serialized.contains("install-target"), "serialized should contain 'install-target', got: {serialized}");

        let tang_server_yaml = "tang-server";
        let deserialized: HostRole = serde_yaml::from_str(tang_server_yaml).unwrap();
        assert_eq!(deserialized, HostRole::TangServer);
        let serialized = serde_yaml::to_string(&deserialized).unwrap();
        assert!(serialized.contains("tang-server"), "serialized should contain 'tang-server', got: {serialized}");
    }

    #[test]
    fn test_tang_server_spec_round_trips() {
        let yaml = r#"
kind: tang-server
port: 80
key-directory: /etc/tang/keys
"#;
        let spec: ApplicationSpec = serde_yaml::from_str(yaml).unwrap();
        let ApplicationSpec::TangServer(tang) = &spec else {
            panic!("expected TangServer variant, got {spec:?}");
        };
        assert_eq!(tang.port, 80);
        assert_eq!(tang.key_directory, "/etc/tang/keys");

        // And back to YAML, round-tripping through the closed enum again.
        let back_yaml = serde_yaml::to_string(&spec).unwrap();
        let back: ApplicationSpec = serde_yaml::from_str(&back_yaml).unwrap();
        assert_eq!(back, spec);
    }

    #[test]
    fn test_tang_server_spec_defaults_port() {
        let yaml = r#"
kind: tang-server
key-directory: /etc/tang/keys
"#;
        let spec: ApplicationSpec = serde_yaml::from_str(yaml).unwrap();
        let ApplicationSpec::TangServer(tang) = spec else {
            panic!("expected TangServer variant");
        };
        assert_eq!(tang.port, 80, "fleet Tang binds port 80 by default");
    }

    #[test]
    fn test_all_new_axes_omit_when_default() {
        // Cross-version rollback safety (same discipline as
        // test_plain_luks_host_omits_storage_keys / StorageMode::is_default): a
        // stock len-serv host — arch defaulted amd64, role install-target, no
        // firmware quirks, no hooks — must serialize WITHOUT any of the four
        // new keys, so a rolled-back control binary (whose deny_unknown_fields
        // InstallationConfig predates these fields) still parses the placed
        // file byte-for-byte as it did before this brief.
        let cfg = InstallationConfig::for_len_serv_003();
        assert_eq!(cfg.arch, Arch::Amd64, "fixture is amd64");
        assert_eq!(cfg.role, HostRole::InstallTarget, "fixture is install-target");
        assert!(cfg.firmware_quirks.is_empty(), "fixture has no firmware quirks");
        assert!(cfg.hooks.is_empty(), "fixture has no hooks");
        let yaml = serde_yaml::to_string(&cfg).unwrap();
        // Match on the YAML key form (`\nkey:`), not a bare substring — several
        // existing keys (`debootstrap_mirror: http://archive...`) contain
        // "arch" as a substring of unrelated text.
        assert!(!yaml.contains("\narch:"), "a default-arch host must omit the arch key entirely, got:\n{yaml}");
        assert!(!yaml.contains("\nrole:"), "an install-target host must omit the role key entirely, got:\n{yaml}");
        assert!(
            !yaml.contains("\nfirmware_quirks:"),
            "a quirk-free host must omit the firmware_quirks key entirely, got:\n{yaml}"
        );
        assert!(!yaml.contains("\nhooks:"), "a hook-free host must omit the hooks key entirely, got:\n{yaml}");
    }

    #[test]
    fn test_non_default_axes_all_serialize_and_round_trip() {
        // The inverse guard: when the four axes carry non-default values, each
        // key must actually appear (else the installer would silently fall
        // back to the amd64/install-target/no-quirk/no-hook defaults).
        let mut cfg = InstallationConfig::for_len_serv_003();
        cfg.arch = Arch::Arm64;
        cfg.role = HostRole::TangServer;
        cfg.firmware_quirks = vec![crate::network::ssh_installer::components::firmware_quirks::FirmwareQuirk::GrubRemovableFallback];
        let mut hooks = Hooks::default();
        hooks.pre_phase.insert(
            crate::network::ssh_installer::components::hooks::Phase::DiskPreparation,
            vec![crate::network::ssh_installer::components::hooks::HookStep {
                run: "echo hi".to_string(),
                chroot: false,
            }],
        );
        cfg.hooks = hooks;

        let yaml = serde_yaml::to_string(&cfg).unwrap();
        assert!(yaml.contains("arch: arm64"), "arm64 arch must serialize, got:\n{yaml}");
        assert!(yaml.contains("role: tang-server"), "tang-server role must serialize, got:\n{yaml}");
        assert!(yaml.contains("firmware_quirks"), "non-empty firmware_quirks must serialize, got:\n{yaml}");
        assert!(yaml.contains("hooks"), "non-empty hooks must serialize, got:\n{yaml}");

        let back: InstallationConfig = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(back.arch, cfg.arch);
        assert_eq!(back.role, cfg.role);
        assert_eq!(back.firmware_quirks, cfg.firmware_quirks);
        assert_eq!(back.hooks, cfg.hooks);
    }

    #[test]
    fn test_tang_server_spec_unknown_field_rejected() {
        let yaml = r#"
kind: tang-server
key-directory: /etc/tang/keys
typo_field: oops
"#;
        let err = serde_yaml::from_str::<ApplicationSpec>(yaml).unwrap_err();
        assert!(
            err.to_string().contains("typo_field"),
            "error must name the unknown field: {err}"
        );
    }
}
