// file: crates/uaa-core/src/profile/lower.rs
// version: 1.0.0
// guid: 74997d1d-8349-4aaf-ac8a-b6ec886492a1
// last-edited: 2026-07-23

//! Pure authoring->flat-wire bridge (PS-LOWER-12).
//!
//! [`lower`] takes a RESOLVED [`InstallationConfigPartial`] — the output of
//! `merge()` BEFORE flattening (group-defaults already resolved over
//! host-overrides into one fully-populated partial) — and produces a
//! concrete [`InstallationConfig`]. Pure and total: no I/O, no `Result`, no
//! panics on any well-typed input. Modeled on
//! [`super::super::network::ssh_installer::layout::plan_layout`]'s pure
//! planner/applier split — `lower` is the planner, the installer pipeline is
//! the applier.
//!
//! ## Field map
//!
//! | nested component field                        | flat wire field (`InstallationConfig`)      |
//! |-------------------------------------------------|----------------------------------------------|
//! | `network.interface`                              | `network_interface`                           |
//! | `network.addressing = Dhcp`                       | `network_address="dhcp"`, `network_gateway=""`|
//! | `network.addressing = Static{address,gateway}`    | `network_address=address`, `network_gateway=gateway` |
//! | `network.search`                                 | `network_search`                              |
//! | `network.nameservers`                            | `network_nameservers`                         |
//! | `network.renderer`                               | `network_renderer`                            |
//! | `base_image.release` (double-option preserved)   | `debootstrap_release`                         |
//! | `base_image.mirror` (double-option preserved)    | `debootstrap_mirror`                          |
//! | `base_image.initramfs`                           | `initramfs_type`                              |
//! | `unlock_policy.tang.servers`                     | `tang_servers`                                |
//! | `unlock_policy.tang.threshold`                   | `tang_threshold`                              |
//! | `unlock_policy.tpm2_pin.pin` (double-option preserved) | `tpm2_pin`                              |
//! | `unlock_policy.tpm2_pin.pcr_ids`                 | `tpm2_pcr_ids`                                |
//! | `unlock_policy.tpm2_pin.enroll`                  | `enroll_tpm2`                                 |
//! | `unlock_policy.fido2_expected`                   | `expect_fido2`                                |
//! | `disk_layout = SingleLuks(spec)`                 | `storage_mode=PlainLuks`, `disk_device=spec.disk_device` |
//! | `disk_layout = ZfsNativeKeystore(spec)`          | `storage_mode=NativeKeystore`, `disks=spec.disks` |
//! | `arch`/`role`/`firmware_quirks`/`hooks`          | copy through unchanged                        |
//!
//! When a nested component (or a leaf within it) is absent, `lower` falls
//! back to the corresponding existing flat field on the resolved partial, so
//! a flat-authored host (no nested components at all — every len-serv host
//! today) still lowers correctly and byte-identically.
//!
//! ## Dropped fields (never lowered — inert until PS-INSTALLER-29)
//!
//! `disk_layout`'s `esp_size`/`reset_size`/`bpool_size`/`reset_enabled` and
//! `base_image.fallback_mirror` have no `InstallationConfig` field to lower
//! into. `unlock_policy.tpm2_clevis_peer` is authoring/validate-only (the
//! D2-B clevis TPM2 peer share is storage_mode-derived by the installer, not
//! a profile input) — see `unlock_policy.rs`'s module doc.
//!
//! ## Secrets
//!
//! `luks_key`, `root_password`, `tpm2_pin`, and `install_ca_cert` are copied
//! through byte-for-byte, including a literal `REPLACE_AT_PLACE_TIME`
//! placeholder token — `lower` never inspects or transforms secret content.

use super::components::network::Addressing;
use super::{DiskLayoutPartial, InstallationConfigPartial};
use crate::network::ssh_installer::config::{
    DiskSpec, InitramfsType, InstallationConfig, StorageMode, TangServer,
};

/// Lowers a RESOLVED authoring-time partial to the flat wire config the
/// installer pipeline consumes. Pure and total.
pub fn lower(resolved: &InstallationConfigPartial) -> InstallationConfig {
    let defaults = wire_defaults();

    let (
        network_interface,
        network_search,
        network_nameservers,
        network_renderer,
        network_address,
        network_gateway,
    ) = lower_network(resolved, &defaults);
    let (debootstrap_release, debootstrap_mirror, initramfs_type) =
        lower_base_image(resolved, &defaults);
    let (tang_servers, tang_threshold, tpm2_pin, tpm2_pcr_ids, enroll_tpm2, expect_fido2) =
        lower_unlock_policy(resolved, &defaults);
    let (storage_mode, disk_device, disks) = lower_disk_layout(resolved);

    InstallationConfig {
        hostname: resolved.hostname.clone().unwrap_or_default(),
        disk_device,
        timezone: resolved.timezone.clone().unwrap_or_default(),
        luks_key: resolved.luks_key.clone().unwrap_or_default(),
        root_password: resolved.root_password.clone().unwrap_or_default(),
        network_interface,
        network_address,
        network_gateway,
        network_search,
        network_nameservers,
        network_renderer,
        debootstrap_release,
        debootstrap_mirror,
        initramfs_type,
        tang_servers,
        tang_threshold,
        ssh_authorized_keys: resolved.ssh_authorized_keys.clone().unwrap_or_default(),
        enroll_tpm2,
        tpm2_pin,
        tpm2_pcr_ids,
        expect_fido2,
        install_ca_cert: resolved
            .install_ca_cert
            .clone()
            .unwrap_or_else(|| defaults.install_ca_cert.clone()),
        applications: resolved.applications.clone().unwrap_or_default(),
        storage_mode,
        disks,
        arch: resolved.arch.unwrap_or_default(),
        role: resolved.role.unwrap_or_default(),
        firmware_quirks: resolved.firmware_quirks.clone().unwrap_or_default(),
        hooks: resolved.hooks.clone().unwrap_or_default(),
    }
}

/// Lowers the `network` component (falling back to the flat
/// `network_*` fields for any absent leaf) into
/// `(interface, search, nameservers, renderer, address, gateway)`.
fn lower_network(
    resolved: &InstallationConfigPartial,
    defaults: &InstallationConfig,
) -> (String, String, Vec<String>, String, String, String) {
    let net = resolved.network.as_ref();

    let interface = net
        .and_then(|n| n.interface.clone())
        .or_else(|| resolved.network_interface.clone())
        .unwrap_or_default();

    let search = net
        .and_then(|n| n.search.clone())
        .or_else(|| resolved.network_search.clone())
        .unwrap_or_default();

    let nameservers = net
        .and_then(|n| n.nameservers.clone())
        .or_else(|| resolved.network_nameservers.clone())
        .unwrap_or_default();

    let renderer = net
        .and_then(|n| n.renderer.clone())
        .or_else(|| resolved.network_renderer.clone())
        .unwrap_or_else(|| defaults.network_renderer.clone());

    let (address, gateway) = match net.and_then(|n| n.addressing.clone()) {
        Some(Addressing::Dhcp) => ("dhcp".to_string(), String::new()),
        Some(Addressing::Static { address, gateway }) => (address, gateway),
        None => (
            resolved.network_address.clone().unwrap_or_default(),
            resolved.network_gateway.clone().unwrap_or_default(),
        ),
    };

    (interface, search, nameservers, renderer, address, gateway)
}

/// Lowers the `base_image` component (falling back to the flat
/// `debootstrap_*`/`initramfs_type` fields for any absent leaf) into
/// `(debootstrap_release, debootstrap_mirror, initramfs_type)`. The
/// double-Option inherit-vs-explicit-none distinction on `release`/`mirror`
/// is preserved: a nested `Some(None)` (explicit no override) wins over the
/// flat fallback and flattens to `None`, same as an unset nested leaf
/// falling back to an unset flat field.
fn lower_base_image(
    resolved: &InstallationConfigPartial,
    defaults: &InstallationConfig,
) -> (Option<String>, Option<String>, InitramfsType) {
    let base_image = resolved.base_image.as_ref();

    let release = base_image
        .and_then(|b| b.release.clone())
        .or_else(|| resolved.debootstrap_release.clone())
        .flatten();

    let mirror = base_image
        .and_then(|b| b.mirror.clone())
        .or_else(|| resolved.debootstrap_mirror.clone())
        .flatten();

    let initramfs_type = base_image
        .and_then(|b| b.initramfs.clone())
        .or_else(|| resolved.initramfs_type.clone())
        .unwrap_or_else(|| defaults.initramfs_type.clone());

    (release, mirror, initramfs_type)
}

/// Lowers the `unlock_policy` component (falling back to the flat
/// `tang_servers`/`tang_threshold`/`tpm2_pin`/`tpm2_pcr_ids`/`enroll_tpm2`/
/// `expect_fido2` fields for any absent leaf) into
/// `(tang_servers, tang_threshold, tpm2_pin, tpm2_pcr_ids, enroll_tpm2, expect_fido2)`.
/// `tpm2_pin`'s double-Option inherit-vs-explicit-none distinction is
/// preserved the same way as `base_image.release`/`mirror` above.
/// `tpm2_clevis_peer` is intentionally never read here — see the module doc.
fn lower_unlock_policy(
    resolved: &InstallationConfigPartial,
    defaults: &InstallationConfig,
) -> (Vec<TangServer>, u8, Option<String>, String, bool, bool) {
    let unlock_policy = resolved.unlock_policy.as_ref();
    let tang = unlock_policy.and_then(|u| u.tang.as_ref());
    let tpm2_pin_partial = unlock_policy.and_then(|u| u.tpm2_pin.as_ref());

    let tang_servers = tang
        .and_then(|t| t.servers.clone())
        .or_else(|| resolved.tang_servers.clone())
        .unwrap_or_default();

    let tang_threshold = tang
        .and_then(|t| t.threshold)
        .or(resolved.tang_threshold)
        .unwrap_or(defaults.tang_threshold);

    let tpm2_pin = tpm2_pin_partial
        .and_then(|t| t.pin.clone())
        .or_else(|| resolved.tpm2_pin.clone())
        .flatten();

    let tpm2_pcr_ids = tpm2_pin_partial
        .and_then(|t| t.pcr_ids.clone())
        .or_else(|| resolved.tpm2_pcr_ids.clone())
        .unwrap_or_else(|| defaults.tpm2_pcr_ids.clone());

    let enroll_tpm2 = tpm2_pin_partial
        .and_then(|t| t.enroll)
        .or(resolved.enroll_tpm2)
        .unwrap_or(defaults.enroll_tpm2);

    let expect_fido2 = unlock_policy
        .and_then(|u| u.fido2_expected)
        .or(resolved.expect_fido2)
        .unwrap_or(defaults.expect_fido2);

    (
        tang_servers,
        tang_threshold,
        tpm2_pin,
        tpm2_pcr_ids,
        enroll_tpm2,
        expect_fido2,
    )
}

/// Lowers the `disk_layout` component (falling back to the flat
/// `storage_mode`/`disk_device`/`disks` fields when absent) into
/// `(storage_mode, disk_device, disks)`. `SingleLuks`'s
/// `esp_size`/`reset_size`/`bpool_size`/`reset_enabled` are intentionally
/// dropped — see the module doc.
fn lower_disk_layout(resolved: &InstallationConfigPartial) -> (StorageMode, String, Vec<DiskSpec>) {
    match resolved.disk_layout.as_ref() {
        Some(DiskLayoutPartial::SingleLuks(spec)) => {
            let disk_device = spec
                .disk_device
                .clone()
                .or_else(|| resolved.disk_device.clone())
                .unwrap_or_default();
            (
                StorageMode::PlainLuks,
                disk_device,
                resolved.disks.clone().unwrap_or_default(),
            )
        }
        Some(DiskLayoutPartial::ZfsNativeKeystore(spec)) => {
            let disks = spec
                .disks
                .clone()
                .or_else(|| resolved.disks.clone())
                .unwrap_or_default();
            (
                StorageMode::NativeKeystore,
                resolved.disk_device.clone().unwrap_or_default(),
                disks,
            )
        }
        None => (
            resolved.storage_mode.clone().unwrap_or_default(),
            resolved.disk_device.clone().unwrap_or_default(),
            resolved.disks.clone().unwrap_or_default(),
        ),
    }
}

/// Builds an `InstallationConfig` from a placeholder JSON document so this
/// module can read back its literal serde defaults (tang threshold, PCR
/// ids, TPM2/FIDO2 opt-in, network renderer, install CA placeholder)
/// instead of duplicating the numbers here. Mirrors
/// `merge.rs::resolved_defaults` (kept as a separate copy, not shared, so
/// the two additive wave-1/2 files never collide on a shared private
/// helper — see the profile-system README's wave/collision rules). The 10
/// fields with no default get throwaway placeholders; `lower` never reads
/// them off this value.
fn wire_defaults() -> InstallationConfig {
    serde_json::from_str(
        r#"{
            "hostname": "",
            "disk_device": "",
            "timezone": "",
            "luks_key": "",
            "root_password": "",
            "network_interface": "",
            "network_address": "",
            "network_gateway": "",
            "network_search": "",
            "network_nameservers": []
        }"#,
    )
    .expect(
        "InstallationConfig's placeholder document must deserialize: every \
         required field is supplied above and every other field carries a \
         serde default",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::ssh_installer::components::disk_layout::{
        NativeKeystoreSpecPartial, SingleLuksSpecPartial,
    };
    use crate::network::ssh_installer::components::firmware_quirks::FirmwareQuirk;
    use crate::network::ssh_installer::config::{Arch, DiskRole, HostRole};
    use crate::profile::components::base_image::BaseImagePartial;
    use crate::profile::components::network::NetworkConfigPartial;
    use crate::profile::components::unlock_policy::{
        TangSssPartial, Tpm2PinPartial, UnlockPolicyPartial,
    };

    /// A partial with every leaf that has a sensible test value set, via the
    /// nested components only (no flat authoring), to exercise the
    /// "component wins" path of every mapping in the table above.
    fn fully_populated_via_components() -> InstallationConfigPartial {
        InstallationConfigPartial {
            hostname: Some("test-host".to_string()),
            timezone: Some("America/New_York".to_string()),
            luks_key: Some("REPLACE_AT_PLACE_TIME".to_string()),
            root_password: Some("REPLACE_AT_PLACE_TIME".to_string()),
            ssh_authorized_keys: Some(vec!["ssh-ed25519 AAAA...".to_string()]),
            install_ca_cert: Some("REPLACE_AT_PLACE_TIME".to_string()),
            applications: Some(Vec::new()),
            network: Some(NetworkConfigPartial {
                interface: Some("eth0".to_string()),
                addressing: Some(Addressing::Static {
                    address: "192.0.2.10/24".to_string(),
                    gateway: "192.0.2.1".to_string(),
                }),
                search: Some("example.internal".to_string()),
                nameservers: Some(vec!["192.0.2.53".to_string()]),
                renderer: Some("NetworkManager".to_string()),
            }),
            base_image: Some(BaseImagePartial {
                release: Some(Some("jammy".to_string())),
                mirror: Some(Some("http://archive.ubuntu.com/ubuntu/".to_string())),
                initramfs: Some(InitramfsType::InitramfsTools),
                fallback_mirror: Some("http://old-releases.ubuntu.com/ubuntu/".to_string()),
            }),
            unlock_policy: Some(UnlockPolicyPartial {
                tang: Some(TangSssPartial {
                    servers: Some(vec![TangServer {
                        url: "http://tang1.example.internal".to_string(),
                    }]),
                    threshold: Some(1),
                }),
                tpm2_pin: Some(Tpm2PinPartial {
                    pin: Some(Some("1234".to_string())),
                    pcr_ids: Some("0,7".to_string()),
                    enroll: Some(true),
                }),
                tpm2_clevis_peer: Some(true),
                fido2_expected: Some(false),
            }),
            disk_layout: Some(DiskLayoutPartial::SingleLuks(SingleLuksSpecPartial {
                esp_size: Some("1G".to_string()),
                reset_size: Some("8G".to_string()),
                bpool_size: Some("3G".to_string()),
                disk_device: Some("/dev/nvme0n1".to_string()),
                reset_enabled: Some(false),
            })),
            arch: Some(Arch::Arm64),
            role: Some(HostRole::TangServer),
            firmware_quirks: Some(vec![FirmwareQuirk::GrubRemovableFallback]),
            ..Default::default()
        }
    }

    #[test]
    fn test_lower_all_fields_set_via_components_no_panic() {
        let resolved = fully_populated_via_components();
        let config = lower(&resolved);

        assert_eq!(config.hostname, "test-host");
        assert_eq!(config.network_interface, "eth0");
        assert_eq!(config.network_address, "192.0.2.10/24");
        assert_eq!(config.network_gateway, "192.0.2.1");
        assert_eq!(config.network_search, "example.internal");
        assert_eq!(config.network_nameservers, vec!["192.0.2.53".to_string()]);
        assert_eq!(config.network_renderer, "NetworkManager");
        assert_eq!(config.debootstrap_release, Some("jammy".to_string()));
        assert_eq!(
            config.debootstrap_mirror,
            Some("http://archive.ubuntu.com/ubuntu/".to_string())
        );
        assert_eq!(config.initramfs_type, InitramfsType::InitramfsTools);
        assert_eq!(config.tang_servers.len(), 1);
        assert_eq!(config.tang_servers[0].url, "http://tang1.example.internal");
        assert_eq!(config.tang_threshold, 1);
        assert_eq!(config.tpm2_pin, Some("1234".to_string()));
        assert_eq!(config.tpm2_pcr_ids, "0,7");
        assert!(config.enroll_tpm2);
        assert!(!config.expect_fido2);
        assert_eq!(config.storage_mode, StorageMode::PlainLuks);
        assert_eq!(config.disk_device, "/dev/nvme0n1");
        assert_eq!(config.arch, Arch::Arm64);
        assert_eq!(config.role, HostRole::TangServer);
        assert_eq!(
            config.firmware_quirks,
            vec![FirmwareQuirk::GrubRemovableFallback]
        );
    }

    #[test]
    fn test_lower_only_required_flat_fields_no_panic() {
        let resolved = InstallationConfigPartial {
            hostname: Some("min-host".to_string()),
            disk_device: Some("/dev/sda".to_string()),
            timezone: Some("UTC".to_string()),
            luks_key: Some("REPLACE_AT_PLACE_TIME".to_string()),
            root_password: Some("REPLACE_AT_PLACE_TIME".to_string()),
            network_interface: Some("eth0".to_string()),
            network_address: Some("dhcp".to_string()),
            network_gateway: Some(String::new()),
            network_search: Some(String::new()),
            network_nameservers: Some(Vec::new()),
            ..Default::default()
        };

        let config = lower(&resolved);

        assert_eq!(config.hostname, "min-host");
        assert_eq!(config.disk_device, "/dev/sda");
        // Unset defaulted fields fall back to InstallationConfig's own serde
        // defaults, not empty/zero garbage.
        assert_eq!(config.network_renderer, "networkd");
        assert_eq!(config.tang_threshold, 2);
        assert_eq!(config.tpm2_pcr_ids, "7");
        assert!(config.enroll_tpm2);
        assert!(config.expect_fido2);
        assert_eq!(config.initramfs_type, InitramfsType::Dracut);
        assert_eq!(config.storage_mode, StorageMode::PlainLuks);
        assert_eq!(config.arch, Arch::Amd64);
        assert_eq!(config.role, HostRole::InstallTarget);
        assert!(config.firmware_quirks.is_empty());
        assert!(config.tang_servers.is_empty());
        assert_eq!(config.tpm2_pin, None);
    }

    #[test]
    fn test_lower_native_keystore_disks_no_panic() {
        let resolved = InstallationConfigPartial {
            hostname: Some("u1".to_string()),
            disk_layout: Some(DiskLayoutPartial::ZfsNativeKeystore(
                NativeKeystoreSpecPartial {
                    disks: Some(vec![
                        DiskSpec {
                            id: "/dev/disk/by-id/nvme-eui.001".to_string(),
                            role: DiskRole::System,
                        },
                        DiskSpec {
                            id: "/dev/disk/by-id/nvme-eui.002".to_string(),
                            role: DiskRole::Special,
                        },
                    ]),
                },
            )),
            ..Default::default()
        };

        let config = lower(&resolved);

        assert_eq!(config.storage_mode, StorageMode::NativeKeystore);
        assert_eq!(config.disks.len(), 2);
        assert_eq!(config.disks[0].id, "/dev/disk/by-id/nvme-eui.001");
    }

    #[test]
    fn test_lower_flat_only_fallback_matches_direct_flat_fields() {
        // No nested components at all — exactly today's every-committed-host
        // shape. `lower` must reproduce the flat fields byte-for-byte (the
        // len-serv PlainLuks-path acceptance bar).
        let resolved = InstallationConfigPartial {
            hostname: Some("len-serv-003".to_string()),
            disk_device: Some("/dev/nvme0n1".to_string()),
            timezone: Some("America/New_York".to_string()),
            luks_key: Some("REPLACE_AT_PLACE_TIME".to_string()),
            root_password: Some("REPLACE_AT_PLACE_TIME".to_string()),
            network_interface: Some("enp1s0f0".to_string()),
            network_address: Some("172.16.3.96/23".to_string()),
            network_gateway: Some("172.16.2.1".to_string()),
            network_search: Some("jf.local".to_string()),
            network_nameservers: Some(vec!["172.16.2.1".to_string()]),
            tang_servers: Some(vec![TangServer {
                url: "http://tang1.example.internal".to_string(),
            }]),
            tang_threshold: Some(1),
            tpm2_pin: Some(Some("9999".to_string())),
            storage_mode: Some(StorageMode::PlainLuks),
            ..Default::default()
        };

        let config = lower(&resolved);

        assert_eq!(config.hostname, "len-serv-003");
        assert_eq!(config.disk_device, "/dev/nvme0n1");
        assert_eq!(config.network_interface, "enp1s0f0");
        assert_eq!(config.network_address, "172.16.3.96/23");
        assert_eq!(config.network_gateway, "172.16.2.1");
        assert_eq!(config.network_search, "jf.local");
        assert_eq!(config.network_nameservers, vec!["172.16.2.1".to_string()]);
        assert_eq!(config.tang_servers.len(), 1);
        assert_eq!(config.tang_threshold, 1);
        assert_eq!(config.tpm2_pin, Some("9999".to_string()));
        assert_eq!(config.storage_mode, StorageMode::PlainLuks);
    }

    #[test]
    fn test_tpm2_pin_explicit_none_distinct_from_inherit() {
        // Explicit-none via the nested component: unlock_policy.tpm2_pin.pin
        // = Some(None) must lower to `None`, same end value as inherit, but
        // reached via a distinct path that must NOT fall through to a flat
        // tpm2_pin the way an absent nested leaf would.
        let explicit_none = InstallationConfigPartial {
            tpm2_pin: Some(Some("should-be-ignored".to_string())), // flat fallback present
            unlock_policy: Some(UnlockPolicyPartial {
                tpm2_pin: Some(Tpm2PinPartial {
                    pin: Some(None), // explicit no-PIN wins over the flat fallback
                    ..Default::default()
                }),
                ..Default::default()
            }),
            ..Default::default()
        };
        assert_eq!(lower(&explicit_none).tpm2_pin, None);

        // Inherit case: no nested unlock_policy at all, so the flat field
        // (itself unset) is consulted and also yields `None` — but via the
        // fallback path, not the explicit-none path.
        let inherit = InstallationConfigPartial {
            tpm2_pin: None,
            unlock_policy: None,
            ..Default::default()
        };
        assert_eq!(lower(&inherit).tpm2_pin, None);

        // Prove the two inputs are themselves distinct (this is the actual
        // "distinctly from an inherit case" assertion: two different
        // resolved-partial shapes, both correctly lowering to `None`,
        // instead of one silently masking the other).
        assert_ne!(explicit_none, inherit);

        // And a flat-fallback inherit that DOES have a value set proves the
        // fallback path is live (not just always-None by coincidence).
        let inherit_with_flat_value = InstallationConfigPartial {
            tpm2_pin: Some(Some("4321".to_string())),
            unlock_policy: None,
            ..Default::default()
        };
        assert_eq!(
            lower(&inherit_with_flat_value).tpm2_pin,
            Some("4321".to_string())
        );
    }

    #[test]
    fn test_replace_at_place_time_survives_lower_unchanged() {
        const TOKEN: &str = "REPLACE_AT_PLACE_TIME";
        let resolved = InstallationConfigPartial {
            luks_key: Some(TOKEN.to_string()),
            root_password: Some(TOKEN.to_string()),
            tpm2_pin: Some(Some(TOKEN.to_string())),
            install_ca_cert: Some(TOKEN.to_string()),
            ..Default::default()
        };

        let config = lower(&resolved);

        assert_eq!(config.luks_key, TOKEN);
        assert_eq!(config.root_password, TOKEN);
        assert_eq!(config.tpm2_pin, Some(TOKEN.to_string()));
        assert_eq!(config.install_ca_cert, TOKEN);
    }

    #[test]
    fn test_native_keystore_component_drops_disk_sizes_and_reset_and_peer() {
        let resolved = InstallationConfigPartial {
            disk_layout: Some(DiskLayoutPartial::ZfsNativeKeystore(
                NativeKeystoreSpecPartial {
                    disks: Some(vec![DiskSpec {
                        id: "/dev/disk/by-id/nvme-eui.001".to_string(),
                        role: DiskRole::System,
                    }]),
                },
            )),
            base_image: Some(BaseImagePartial {
                fallback_mirror: Some("http://old-releases.ubuntu.com/ubuntu/".to_string()),
                ..Default::default()
            }),
            unlock_policy: Some(UnlockPolicyPartial {
                tpm2_clevis_peer: Some(true),
                ..Default::default()
            }),
            ..Default::default()
        };

        let config = lower(&resolved);

        assert_eq!(config.storage_mode, StorageMode::NativeKeystore);
        assert_eq!(config.disks.len(), 1);
        // `InstallationConfig` has no esp_size/reset_size/bpool_size/
        // reset_enabled/fallback_mirror/tpm2_clevis_peer fields at all — the
        // struct shape itself is the proof these never lower. This
        // round-trip through serde_json is a belt-and-suspenders check that
        // no such keys leak into the serialized wire form either.
        let json = serde_json::to_value(&config).unwrap();
        let obj = json.as_object().unwrap();
        for dropped in [
            "esp_size",
            "reset_size",
            "bpool_size",
            "reset_enabled",
            "fallback_mirror",
            "tpm2_clevis_peer",
        ] {
            assert!(
                !obj.contains_key(dropped),
                "dropped field `{dropped}` leaked into wire output"
            );
        }
    }
}
