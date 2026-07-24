<!-- file: changelog.d/ps-validate-14.md -->
<!-- version: 1.0.0 -->
<!-- guid: 7c6e2a1d-9b4f-4e8a-9c3d-2f5b8a1e6d47 -->
<!-- last-edited: 2026-07-23 -->

### Added

- `validate_resolved(&InstallationConfig) -> Result<()>` in `profile/validate.rs`: a post-merge composition-legality sibling to the existing pre-merge `validate(groups, profiles)`. Checks storage-mode/arch/disk consistency for `NativeKeystore`, Tang SSS threshold range, the arm64 vs. `GrubRemovableFallback` firmware quirk conflict, and role-specific requirements (`TangServer` needs a `TangServer` application; `InstallTarget` needs both a disk plan and an unlock path) (PS-VALIDATE-14)
