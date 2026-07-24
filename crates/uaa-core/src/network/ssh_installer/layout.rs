// file: crates/uaa-core/src/network/ssh_installer/layout.rs
// version: 2.0.1
// guid: 95e0d672-e175-4029-b56a-a025267f71d2
// last-edited: 2026-07-23

//! Pure partition planner for the [`StorageMode::NativeKeystore`] (U1) layout.
//!
//! Given a roster of role-tagged disks, this computes *what* partitions each
//! disk needs — it does **no** I/O. The applier ([`super::disk_native`]) turns a
//! [`PartitionPlan`] into `sgdisk` invocations against real devices.
//!
//! Topology (design §2, **revised 2026-07-23**). The X10DSC+ firmware cannot
//! boot from NVMe — it enumerates the Optanes for the OS but its UEFI boot
//! manager has no NVMe entry (proved on real hardware with a clean ext4 test
//! install). So the ESP + `bpool` moved **off** the Optane onto the SATA SSDs,
//! which the firmware *can* boot. The Optanes keep their ideal job — the
//! `special` (metadata) vdev:
//!
//! | Device            | p1                | p2            | p3                 |
//! |-------------------|-------------------|---------------|--------------------|
//! | System SSD (2×)   | ESP (1 G, EF00)   | bpool (2 G)   | rpool data (rest)  |
//! | Special Optane(2×)| special (6 G)     | — free —      |                    |
//!
//! - `bpool  = mirror(System0.p2, System1.p2)` — unencrypted `/boot`, bootable
//!   SATA so GRUB/shim load from a disk the firmware enumerates.
//! - `rpool  = mirror(System0.p3, System1.p3) [data]
//!            + mirror(Special0.p1, Special1.p1) [special]`.
//! - The Optane `special` partition is **half the drive** ([`SPECIAL_SIZE_BYTES`]);
//!   the remainder is deliberately left unpartitioned, reserved for a future
//!   spinning-disk array's own special vdev (operator decision 2026-07-23).
//! - `special_small_blocks=0` (metadata only) — the data pool is itself SSD, so
//!   there is no small-file latency to offload onto the Optane (only worth it in
//!   front of spinning rust). Set in [`super::zfs_native`].
//! - Both System ESPs are registered in NVRAM independently; they are **never**
//!   block-level RAID-mirrored (a later phase syncs them file-by-file — see
//!   design §D7.3).
//!
//! ESP and bpool are [`PartSize::Fixed`] because they become ZFS *mirror
//! members* (both System SSDs must contribute byte-identical partitions), and
//! the Optane `special` is Fixed too — half-disk, mirror-symmetric. Only the
//! System `data` partition is [`PartSize::Remainder`] ("rest of disk"), so disk
//! capacity never enters this pure planner (which also sidesteps the
//! decimal-vs-GiB sizing trap).

use super::config::{DiskRole, DiskSpec};
use std::collections::HashSet;
use std::fmt;

/// 1 GiB, in bytes.
const GIB: u64 = 1024 * 1024 * 1024;

/// ESP size — 1 GiB FAT32, registered in NVRAM. Fixed so both mirror ESPs match.
pub const ESP_SIZE_BYTES: u64 = GIB;
/// bpool member size — 2 GiB plaintext `/boot`. Fixed for mirror parity.
pub const BPOOL_SIZE_BYTES: u64 = 2 * GIB;
/// `special` (metadata) vdev member size — 6 GiB, ≈half of the 13.4 GiB Optane.
/// Fixed for mirror parity; the remainder of each Optane is left unpartitioned,
/// reserved for a future spinning-disk array's special vdev (decision
/// 2026-07-23). Metadata-only (`special_small_blocks=0`), so 6 GiB comfortably
/// holds the metadata for a ~2 TB SSD data mirror.
pub const SPECIAL_SIZE_BYTES: u64 = 6 * GIB;

/// What a partition is for. Each kind carries its static GPT typecode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartKind {
    /// EFI System Partition — FAT32, registered in NVRAM. On the System SSD.
    Esp,
    /// `bpool` member — unencrypted `/boot` ZFS pool. On the System SSD.
    Bpool,
    /// `rpool` **data** vdev member — the bulk pool. On the System SSD (p3).
    Data,
    /// `rpool` **special** (allocation-class / metadata) vdev member. On the
    /// Optane (p1), half-provisioned.
    Special,
}

impl PartKind {
    /// The `sgdisk` GPT typecode for this kind. `EF00`/`BE00` match the proven
    /// fleet path (`disk_ops::prepare_disk`); `BF00` (Solaris/ZFS) marks both the
    /// `data` and `special` native-ZFS members, which — unlike the Lenovo path —
    /// have no LUKS layer of their own (encryption is native, at the pool).
    pub fn typecode(self) -> &'static str {
        match self {
            PartKind::Esp => "EF00",
            PartKind::Bpool => "BE00",
            PartKind::Data => "BF00",
            PartKind::Special => "BF00",
        }
    }
}

/// A partition's size: an exact byte count, or "the rest of the disk".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartSize {
    /// Exact size — used for mirror members that must match across disks.
    Fixed(u64),
    /// Whatever is left after the fixed partitions; resolved by sgdisk at apply.
    Remainder,
}

/// One planned partition. `number` is load-bearing: later phases reference
/// `System.p2` (bpool) / `System.p3` (data) / `Special.p1` (special) by it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Partition {
    /// GPT partition number (1-based).
    pub number: u32,
    /// What the partition is for.
    pub kind: PartKind,
    /// How big to make it.
    pub size: PartSize,
    /// GPT partition label (`-c`), unique per disk (`ESP1`, `bpool-0`, …).
    pub label: String,
    /// GPT typecode (`-t`), derived from [`PartKind::typecode`].
    pub typecode: &'static str,
}

/// The plan for one physical disk. Every disk in a NativeKeystore roster now
/// carries partitions — the System SSDs get `ESP + bpool + data`, the Special
/// Optanes get a single half-disk `special` member (no whole-disk vdevs).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiskPlan {
    /// Stable `/dev/disk/by-id/...` path from the [`DiskSpec`].
    pub id: String,
    /// The role this disk plays.
    pub role: DiskRole,
    /// Zero-based index within the role (drives `bpool-0`/`bpool-1`, `data-0`…).
    pub role_index: usize,
    /// Partitions to create, in `sgdisk` order.
    pub partitions: Vec<Partition>,
}

/// A complete partition plan for a [`StorageMode::NativeKeystore`] roster.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PartitionPlan {
    /// System disks first (in roster order), then special disks.
    pub disks: Vec<DiskPlan>,
}

impl PartitionPlan {
    /// Number of ESPs across the plan — must be one per system disk (design §D7.3).
    pub fn esp_count(&self) -> usize {
        self.disks
            .iter()
            .flat_map(|d| &d.partitions)
            .filter(|p| p.kind == PartKind::Esp)
            .count()
    }

    /// The planned System (bootable SATA SSD) disks — ESP + bpool + rpool data.
    pub fn system_disks(&self) -> impl Iterator<Item = &DiskPlan> {
        self.disks.iter().filter(|d| d.role == DiskRole::System)
    }

    /// The planned Special (Optane) disks — the metadata vdev members.
    pub fn special_disks(&self) -> impl Iterator<Item = &DiskPlan> {
        self.disks.iter().filter(|d| d.role == DiskRole::Special)
    }
}

/// Why a roster cannot be planned. Every variant is fail-closed: the design
/// requires **mirrored** bpool/data/special vdevs, so a roster that cannot form
/// those mirrors is an error, never a silently non-redundant pool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LayoutError {
    /// Fewer than two disks of a role — the mirror it forms would be degenerate.
    NotEnoughDisks {
        /// The under-populated role.
        role: DiskRole,
        /// How many were supplied.
        found: usize,
    },
    /// A device id appears more than once in the roster.
    DuplicateDisk(String),
    /// A [`DiskSpec`] carried an empty device id.
    EmptyDiskId,
}

impl fmt::Display for LayoutError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LayoutError::NotEnoughDisks { role, found } => write!(
                f,
                "NativeKeystore needs at least 2 {role:?} disks to mirror, found {found}"
            ),
            LayoutError::DuplicateDisk(id) => write!(f, "duplicate disk id in roster: {id}"),
            LayoutError::EmptyDiskId => write!(f, "roster contains a disk with an empty id"),
        }
    }
}

impl std::error::Error for LayoutError {}

/// The three partitions of a System (bootable SATA SSD) disk, in `sgdisk` order.
/// `disk_index` is the zero-based position among system disks; it disambiguates
/// labels (`ESP1`/`ESP2`, `bpool-0`/`bpool-1`) so no two partitions collide.
fn system_partitions(disk_index: usize) -> Vec<Partition> {
    vec![
        Partition {
            number: 1,
            kind: PartKind::Esp,
            size: PartSize::Fixed(ESP_SIZE_BYTES),
            label: format!("ESP{}", disk_index + 1),
            typecode: PartKind::Esp.typecode(),
        },
        Partition {
            number: 2,
            kind: PartKind::Bpool,
            size: PartSize::Fixed(BPOOL_SIZE_BYTES),
            label: format!("bpool-{disk_index}"),
            typecode: PartKind::Bpool.typecode(),
        },
        Partition {
            number: 3,
            kind: PartKind::Data,
            size: PartSize::Remainder,
            label: format!("data-{disk_index}"),
            typecode: PartKind::Data.typecode(),
        },
    ]
}

/// The single partition of a Special (Optane) disk: a half-disk `special`
/// member. The remaining half is intentionally left unpartitioned (free) for a
/// future spinning-disk array's special vdev.
fn special_partitions(disk_index: usize) -> Vec<Partition> {
    vec![Partition {
        number: 1,
        kind: PartKind::Special,
        size: PartSize::Fixed(SPECIAL_SIZE_BYTES),
        label: format!("special-{disk_index}"),
        typecode: PartKind::Special.typecode(),
    }]
}

/// Plan the partitions for a NativeKeystore disk roster.
///
/// System (SSD) disks get `ESP + bpool + data`; Special (Optane) disks get a
/// single half-disk `special` member. Ordering in the output is all system
/// disks (roster order) then all special disks, each numbered within its role.
///
/// # Errors
/// Returns [`LayoutError`] when the roster has an empty/duplicate id, or cannot
/// form the mirrors the design mandates (fewer than two disks of either role).
pub fn plan_layout(disks: &[DiskSpec]) -> Result<PartitionPlan, LayoutError> {
    // Reject empty and duplicate ids up front — a partitioner that ran against a
    // blank or repeated by-id path would wipe the wrong (or same) device twice.
    let mut seen: HashSet<&str> = HashSet::new();
    for d in disks {
        if d.id.trim().is_empty() {
            return Err(LayoutError::EmptyDiskId);
        }
        if !seen.insert(d.id.as_str()) {
            return Err(LayoutError::DuplicateDisk(d.id.clone()));
        }
    }

    let system: Vec<&DiskSpec> = disks.iter().filter(|d| d.role == DiskRole::System).collect();
    let special: Vec<&DiskSpec> = disks.iter().filter(|d| d.role == DiskRole::Special).collect();

    if system.len() < 2 {
        return Err(LayoutError::NotEnoughDisks {
            role: DiskRole::System,
            found: system.len(),
        });
    }
    if special.len() < 2 {
        return Err(LayoutError::NotEnoughDisks {
            role: DiskRole::Special,
            found: special.len(),
        });
    }

    let mut plan = Vec::with_capacity(disks.len());
    for (i, d) in system.iter().enumerate() {
        plan.push(DiskPlan {
            id: d.id.clone(),
            role: DiskRole::System,
            role_index: i,
            partitions: system_partitions(i),
        });
    }
    for (i, d) in special.iter().enumerate() {
        plan.push(DiskPlan {
            id: d.id.clone(),
            role: DiskRole::Special,
            role_index: i,
            partitions: special_partitions(i),
        });
    }

    Ok(PartitionPlan { disks: plan })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The canonical U1 roster: 2 SSD (system, bootable) + 2 Optane (special),
    /// by-id. Boot lives on the SSDs because the firmware can't boot NVMe.
    fn u1_roster() -> Vec<DiskSpec> {
        vec![
            DiskSpec {
                id: "/dev/disk/by-id/ata-CRUCIAL_0".to_string(),
                role: DiskRole::System,
            },
            DiskSpec {
                id: "/dev/disk/by-id/ata-CRUCIAL_1".to_string(),
                role: DiskRole::System,
            },
            DiskSpec {
                id: "/dev/disk/by-id/nvme-INTEL_OPTANE_0".to_string(),
                role: DiskRole::Special,
            },
            DiskSpec {
                id: "/dev/disk/by-id/nvme-INTEL_OPTANE_1".to_string(),
                role: DiskRole::Special,
            },
        ]
    }

    /// The shape (number/kind/size/typecode) of a partition, ignoring its
    /// per-disk unique label — this is what must be identical across the two
    /// mirror members for them to line up.
    fn shape(p: &Partition) -> (u32, PartKind, PartSize, &'static str) {
        (p.number, p.kind, p.size, p.typecode)
    }

    #[test]
    fn both_system_disks_are_symmetric() {
        let plan = plan_layout(&u1_roster()).expect("valid roster");
        let system: Vec<&DiskPlan> = plan.system_disks().collect();
        assert_eq!(system.len(), 2, "two system disks");

        let shapes0: Vec<_> = system[0].partitions.iter().map(shape).collect();
        let shapes1: Vec<_> = system[1].partitions.iter().map(shape).collect();
        assert_eq!(shapes0, shapes1, "both System SSDs get identical partition shapes");

        // p1=ESP(1G), p2=bpool(2G), p3=data(rest).
        assert_eq!(
            shapes0,
            vec![
                (1, PartKind::Esp, PartSize::Fixed(ESP_SIZE_BYTES), "EF00"),
                (2, PartKind::Bpool, PartSize::Fixed(BPOOL_SIZE_BYTES), "BE00"),
                (3, PartKind::Data, PartSize::Remainder, "BF00"),
            ]
        );
    }

    #[test]
    fn both_special_disks_are_symmetric_half_disk() {
        let plan = plan_layout(&u1_roster()).expect("valid roster");
        let special: Vec<&DiskPlan> = plan.special_disks().collect();
        assert_eq!(special.len(), 2, "two special disks");

        let shapes0: Vec<_> = special[0].partitions.iter().map(shape).collect();
        let shapes1: Vec<_> = special[1].partitions.iter().map(shape).collect();
        assert_eq!(shapes0, shapes1, "both Optanes get identical special shapes");

        // A single half-disk special member; the remainder is left free.
        assert_eq!(
            shapes0,
            vec![(1, PartKind::Special, PartSize::Fixed(SPECIAL_SIZE_BYTES), "BF00")]
        );
    }

    #[test]
    fn special_partition_is_half_disk_fixed_not_remainder() {
        // Guards the "reserve the other half" intent: if this ever regresses to
        // Remainder, the whole Optane would be consumed and the future spinning
        // array would have nowhere to put its own special vdev.
        let plan = plan_layout(&u1_roster()).expect("valid roster");
        let special = plan.special_disks().next().expect("a special disk");
        assert_eq!(special.partitions.len(), 1, "exactly one special partition");
        assert_eq!(
            special.partitions[0].size,
            PartSize::Fixed(SPECIAL_SIZE_BYTES),
            "special must be a fixed half-disk size, never the disk remainder"
        );
    }

    #[test]
    fn labels_are_unique_per_disk() {
        let plan = plan_layout(&u1_roster()).expect("valid roster");
        let system: Vec<&DiskPlan> = plan.system_disks().collect();
        assert_eq!(system[0].partitions[0].label, "ESP1");
        assert_eq!(system[1].partitions[0].label, "ESP2");
        assert_eq!(system[0].partitions[1].label, "bpool-0");
        assert_eq!(system[1].partitions[1].label, "bpool-1");
        assert_eq!(system[0].partitions[2].label, "data-0");
        assert_eq!(system[1].partitions[2].label, "data-1");
        let special: Vec<&DiskPlan> = plan.special_disks().collect();
        assert_eq!(special[0].partitions[0].label, "special-0");
        assert_eq!(special[1].partitions[0].label, "special-1");
    }

    #[test]
    fn exactly_two_esps_on_the_bootable_disks() {
        let plan = plan_layout(&u1_roster()).expect("valid roster");
        assert_eq!(plan.esp_count(), 2, "one ESP per system disk, both in NVRAM");
        // ESPs live only on System (bootable SATA) disks, never on the Optanes.
        for sp in plan.special_disks() {
            assert!(
                sp.partitions.iter().all(|p| p.kind != PartKind::Esp),
                "no ESP on a non-bootable Optane ({})",
                sp.id
            );
        }
    }

    #[test]
    fn rejects_single_system_disk() {
        let roster = vec![
            DiskSpec {
                id: "/dev/disk/by-id/ssd0".to_string(),
                role: DiskRole::System,
            },
            DiskSpec {
                id: "/dev/disk/by-id/optane0".to_string(),
                role: DiskRole::Special,
            },
            DiskSpec {
                id: "/dev/disk/by-id/optane1".to_string(),
                role: DiskRole::Special,
            },
        ];
        assert_eq!(
            plan_layout(&roster),
            Err(LayoutError::NotEnoughDisks {
                role: DiskRole::System,
                found: 1,
            })
        );
    }

    #[test]
    fn rejects_single_special_disk() {
        let roster = vec![
            DiskSpec {
                id: "/dev/disk/by-id/ssd0".to_string(),
                role: DiskRole::System,
            },
            DiskSpec {
                id: "/dev/disk/by-id/ssd1".to_string(),
                role: DiskRole::System,
            },
            DiskSpec {
                id: "/dev/disk/by-id/optane0".to_string(),
                role: DiskRole::Special,
            },
        ];
        assert_eq!(
            plan_layout(&roster),
            Err(LayoutError::NotEnoughDisks {
                role: DiskRole::Special,
                found: 1,
            })
        );
    }

    #[test]
    fn rejects_duplicate_disk_id() {
        let mut roster = u1_roster();
        roster[3].id = roster[2].id.clone();
        assert_eq!(
            plan_layout(&roster),
            Err(LayoutError::DuplicateDisk(roster[2].id.clone()))
        );
    }

    #[test]
    fn rejects_empty_disk_id() {
        let mut roster = u1_roster();
        roster[0].id = "  ".to_string();
        assert_eq!(plan_layout(&roster), Err(LayoutError::EmptyDiskId));
    }
}
