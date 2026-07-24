// file: crates/uaa-core/src/network/ssh_installer/partitions.rs
// version: 1.1.0
// guid: 9e8ed319-c4c1-4e58-8e3b-dc5f8a4f869b
// last-edited: 2026-07-23

//! Suffix-aware partition path construction for the SSH installer.

/// Build the path of partition `n` on `disk`, following the kernel naming
/// rule: insert a `p` separator only when the disk name ends in a digit.
/// `/dev/nvme0n1` -> `/dev/nvme0n1p3`, `/dev/nvme1n1` -> `/dev/nvme1n1p3`,
/// `/dev/sda` -> `/dev/sda3`, `/dev/vda` -> `/dev/vda3`.
pub fn partition_path(disk: &str, n: u32) -> String {
    if disk.chars().last().is_some_and(|c| c.is_ascii_digit()) {
        format!("{disk}p{n}")
    } else {
        format!("{disk}{n}")
    }
}

#[cfg(test)]
mod tests {
    use super::partition_path;

    #[test]
    fn nvme_gets_p_suffix() {
        assert_eq!(partition_path("/dev/nvme0n1", 3), "/dev/nvme0n1p3");
    }

    #[test]
    fn second_nvme_gets_p_suffix() {
        assert_eq!(partition_path("/dev/nvme1n1", 4), "/dev/nvme1n1p4");
    }

    #[test]
    fn sda_no_p_suffix() {
        assert_eq!(partition_path("/dev/sda", 1), "/dev/sda1");
    }

    #[test]
    fn vda_no_p_suffix() {
        assert_eq!(partition_path("/dev/vda", 4), "/dev/vda4");
    }

    #[test]
    fn nested_path_ending_in_digit_gets_p_suffix() {
        assert_eq!(
            partition_path("/dev/disk/by-id/nvme-Data_0", 3),
            "/dev/disk/by-id/nvme-Data_0p3"
        );
    }

    #[test]
    fn empty_disk_does_not_panic() {
        assert_eq!(partition_path("", 1), "1");
    }
}
