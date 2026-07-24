### Added

#### Profile system: base-image authoring sub-struct (BaseImagePartial)

Added new `BaseImagePartial` type in `crates/uaa-core/src/profile/components/base_image.rs` to support Ubuntu base image configuration in host/group profiles. The struct provides double-Option semantics for `release` and `mirror` fields to distinguish between inheritance, explicit nulls, and explicit values. The `initramfs` field reuses the existing `InitramfsType` enum from `ssh_installer::config`, and `fallback_mirror` surfaces the old-releases URL for authoring expressibility.

Fields map to lower-level installer configuration per PS-LOWER-12:
- `release` → `debootstrap_release`
- `mirror` → `debootstrap_mirror`
- `initramfs` → `initramfs_type`
- `fallback_mirror` → (authoring-only, inert until installer reads it)
