// file: crates/uaa-core/src/network/ssh_installer/layout.rs
// version: 1.0.0
// guid: 95e0d672-e175-4029-b56a-a025267f71d2
// last-edited: 2026-07-22

//! Pure partition planner for the [`StorageMode::NativeKeystore`] (U1) layout.
//!
//! Given a roster of role-tagged disks, this computes *what* partitions each
//! disk needs — it does **no** I/O. The applier (a later phase's `partitions.rs`)
//! turns a [`PartitionPlan`] into `sgdisk` invocations against real devices; the
//! VM gate can only exercise that with a bootable artifact, so Phase 1 ships the
//! planner + its assertions and defers the applier.
//!
//! Topology (see `docs/specs/u1-zfs-native-encryption-design.md` §2):
//!
//! | Device        | p1                | p2            | p3                 |
//! |---------------|-------------------|---------------|--------------------|
//! | Optane (2×)   | ESP (1 G, EF00)   | bpool (2 G)   | special (rest)     |
//! | SSD (2×)      | — whole disk (raw ZFS data-vdev member) —                |
//!
//! - `bpool  = mirror(Optane0.p2, Optane1.p2)` — unencrypted `/boot`.
//! - `rpool  = mirror(SSD0, SSD1) [data] + mirror(Optane0.p3, Optane1.p3) [special]`.
//! - Both ESPs are registered in NVRAM independently; they are **never**
//!   mdadm-mirrored (a later phase syncs them file-by-file — see design §D7.3).
//!
//! ESP and bpool are [`PartSize::Fixed`] on purpose: they become ZFS *mirror
//! members*, so both Optanes must contribute byte-identical partitions. The
//! `special` partition is [`PartSize::Remainder`] — sgdisk resolves "rest of
//! disk" at apply time, so disk capacity never enters this pure planner (which
//! also sidesteps the 16 GB-decimal-vs-GiB sizing trap).

use super::config::{DiskRole, DiskSpec};
use std::collections::HashSet;
use std::fmt;

/// 1 GiB, in bytes.
const GIB: u64 = 1024 * 1024 * 1024;

/// ESP size — 1 GiB FAT32, registered in NVRAM. Fixed so both mirror ESPs match.
pub const ESP_SIZE_BYTES: u64 = GIB;
/// bpool member size — 2 GiB plaintext `/boot`. Fixed for mirror parity.
pub const BPOOL_SIZE_BYTES: u64 = 2 * GIB;

/// What a partition is for. Each kind carries its static GPT typecode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartKind {
    /// EFI System Partition — FAT32, registered in NVRAM.
    Esp,
    /// `bpool` member — unencrypted `/boot` ZFS pool.
    Bpool,
    /// `special` (allocation-class) member — rpool metadata mirror.
    Special,
}

impl PartKind {
    /// The `sgdisk` GPT typecode for this kind. `EF00`/`BE00` match the proven
    /// fleet path (`disk_ops::prepare_disk`); `BF00` (Solaris/ZFS root) marks the
    /// native-ZFS `special` member, which — unlike the Lenovo path — has no LUKS
    /// layer of its own (encryption is native, at the pool).
    pub fn typecode(self) -> &'static str {
        match self {
            PartKind::Esp => "EF00",
            PartKind::Bpool => "BE00",
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

/// One planned partition on a system disk. `number` is load-bearing: later phases
/// reference `Optane.p2` (bpool) / `Optane.p3` (special) by this number.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Partition {
    /// GPT partition number (1-based) — p1=ESP, p2=bpool, p3=special.
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

/// The plan for one physical disk. A `Data` (SSD) disk has an empty `partitions`
/// vec — it is consumed whole as a raw ZFS vdev member, with no GPT table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiskPlan {
    /// Stable `/dev/disk/by-id/...` path from the [`DiskSpec`].
    pub id: String,
    /// The role this disk plays.
    pub role: DiskRole,
    /// Zero-based index within the role (drives `bpool-0`/`bpool-1`, `data-0`…).
    pub role_index: usize,
    /// Partitions to create; empty for a whole-disk `Data` member.
    pub partitions: Vec<Partition>,
}

/// A complete partition plan for a [`StorageMode::NativeKeystore`] roster.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PartitionPlan {
    /// System disks first (in roster order), then data disks.
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

    /// The planned system (Optane) disks.
    pub fn system_disks(&self) -> impl Iterator<Item = &DiskPlan> {
        self.disks.iter().filter(|d| d.role == DiskRole::System)
    }

    /// The planned data (SSD) disks.
    pub fn data_disks(&self) -> impl Iterator<Item = &DiskPlan> {
        self.disks.iter().filter(|d| d.role == DiskRole::Data)
    }
}

/// Why a roster cannot be planned. Every variant is fail-closed: the design
/// requires **mirrored** bpool/special/data vdevs, so a roster that cannot form
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

/// The three partitions of a system (Optane) disk, in `sgdisk` order.
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
            kind: PartKind::Special,
            size: PartSize::Remainder,
            label: format!("special-{disk_index}"),
            typecode: PartKind::Special.typecode(),
        },
    ]
}

/// Plan the partitions for a NativeKeystore disk roster.
///
/// System (Optane) disks get `ESP + bpool + special`; data (SSD) disks are
/// whole-disk. Ordering in the output is all system disks (roster order) then all
/// data disks, each numbered within its role.
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
    let data: Vec<&DiskSpec> = disks.iter().filter(|d| d.role == DiskRole::Data).collect();

    if system.len() < 2 {
        return Err(LayoutError::NotEnoughDisks {
            role: DiskRole::System,
            found: system.len(),
        });
    }
    if data.len() < 2 {
        return Err(LayoutError::NotEnoughDisks {
            role: DiskRole::Data,
            found: data.len(),
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
    for (i, d) in data.iter().enumerate() {
        plan.push(DiskPlan {
            id: d.id.clone(),
            role: DiskRole::Data,
            role_index: i,
            partitions: Vec::new(), // whole-disk data vdev member
        });
    }

    Ok(PartitionPlan { disks: plan })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The canonical U1 roster: 2 Optane (system) + 2 SSD (data), by-id.
    fn u1_roster() -> Vec<DiskSpec> {
        vec![
            DiskSpec {
                id: "/dev/disk/by-id/nvme-INTEL_OPTANE_0".to_string(),
                role: DiskRole::System,
            },
            DiskSpec {
                id: "/dev/disk/by-id/nvme-INTEL_OPTANE_1".to_string(),
                role: DiskRole::System,
            },
            DiskSpec {
                id: "/dev/disk/by-id/ata-SSD_0".to_string(),
                role: DiskRole::Data,
            },
            DiskSpec {
                id: "/dev/disk/by-id/ata-SSD_1".to_string(),
                role: DiskRole::Data,
            },
        ]
    }

    /// The shape (number/kind/size/typecode) of a partition, ignoring its
    /// per-disk unique label — this is what must be identical across the two
    /// Optanes for the mirror members to line up.
    fn shape(p: &Partition) -> (u32, PartKind, PartSize, &'static str) {
        (p.number, p.kind, p.size, p.typecode)
    }

    #[test]
    fn both_optanes_are_symmetric() {
        let plan = plan_layout(&u1_roster()).expect("valid roster");
        let system: Vec<&DiskPlan> = plan.system_disks().collect();
        assert_eq!(system.len(), 2, "two system disks");

        let shapes0: Vec<_> = system[0].partitions.iter().map(shape).collect();
        let shapes1: Vec<_> = system[1].partitions.iter().map(shape).collect();
        assert_eq!(shapes0, shapes1, "both Optanes get identical partition shapes");

        // And the exact expected shape: p1=ESP(1G), p2=bpool(2G), p3=special(rest).
        assert_eq!(
            shapes0,
            vec![
                (1, PartKind::Esp, PartSize::Fixed(ESP_SIZE_BYTES), "EF00"),
                (2, PartKind::Bpool, PartSize::Fixed(BPOOL_SIZE_BYTES), "BE00"),
                (3, PartKind::Special, PartSize::Remainder, "BF00"),
            ]
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
        assert_eq!(system[0].partitions[2].label, "special-0");
        assert_eq!(system[1].partitions[2].label, "special-1");
    }

    #[test]
    fn ssds_are_whole_disk() {
        let plan = plan_layout(&u1_roster()).expect("valid roster");
        let data: Vec<&DiskPlan> = plan.data_disks().collect();
        assert_eq!(data.len(), 2, "two data disks");
        for d in data {
            assert!(
                d.partitions.is_empty(),
                "data disk {} is consumed whole, no partitions",
                d.id
            );
        }
    }

    #[test]
    fn exactly_two_esps() {
        let plan = plan_layout(&u1_roster()).expect("valid roster");
        assert_eq!(plan.esp_count(), 2, "one ESP per system disk, both in NVRAM");
    }

    #[test]
    fn rejects_single_system_disk() {
        let roster = vec![
            DiskSpec {
                id: "/dev/disk/by-id/optane".to_string(),
                role: DiskRole::System,
            },
            DiskSpec {
                id: "/dev/disk/by-id/ssd0".to_string(),
                role: DiskRole::Data,
            },
            DiskSpec {
                id: "/dev/disk/by-id/ssd1".to_string(),
                role: DiskRole::Data,
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
    fn rejects_single_data_disk() {
        let roster = vec![
            DiskSpec {
                id: "/dev/disk/by-id/optane0".to_string(),
                role: DiskRole::System,
            },
            DiskSpec {
                id: "/dev/disk/by-id/optane1".to_string(),
                role: DiskRole::System,
            },
            DiskSpec {
                id: "/dev/disk/by-id/ssd0".to_string(),
                role: DiskRole::Data,
            },
        ];
        assert_eq!(
            plan_layout(&roster),
            Err(LayoutError::NotEnoughDisks {
                role: DiskRole::Data,
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
