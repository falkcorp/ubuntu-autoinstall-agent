// file: crates/uaa-core/tests/single_disk_native_keystore_config.rs
// version: 1.0.0
// guid: b5e91c37-4d28-4f60-a1c9-8e3f27b04d15
// last-edited: 2026-07-29

//! Gate: the committed single-disk NativeKeystore VM config must parse and
//! plan a valid single-vdev layout. Guards the exact shape len-serv-003 uses,
//! so a schema drift or a regression of the 2+2 mirror requirement fails here
//! rather than mid-install on hardware.

use uaa_core::network::ssh_installer::config::{DiskRole, InstallationConfig, StorageMode};
use uaa_core::network::ssh_installer::layout::plan_layout;

#[test]
fn vm_native_keystore_config_parses_and_plans_single_disk() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../examples/configs/install/vm-test-native-keystore.yaml"
    );
    let cfg = InstallationConfig::from_yaml_file(path)
        .expect("single-disk native-keystore VM config must parse");

    assert_eq!(cfg.storage_mode, StorageMode::NativeKeystore);
    assert_eq!(cfg.disks.len(), 1, "single-disk roster");
    assert_eq!(cfg.disks[0].role, DiskRole::System);
    assert!(
        cfg.disks[0].id.starts_with("/dev/disk/by-id/"),
        "native-keystore rosters must be by-id (they append -partN): {}",
        cfg.disks[0].id
    );

    let plan = plan_layout(&cfg.disks).expect("1 system + 0 special must be a valid roster");
    assert_eq!(plan.system_disks().count(), 1);
    assert_eq!(plan.special_disks().count(), 0, "no special vdev on one disk");
    assert_eq!(
        plan.system_disks().next().unwrap().partitions.len(),
        3,
        "ESP + bpool + data"
    );
}
