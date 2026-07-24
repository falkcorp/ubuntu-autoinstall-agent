### Added

#### Arch classifier enum (PS-ARCH-07)

Define new `Arch { Amd64, Arm64 }` enum in `ssh_installer::config` with serde
support for kebab-case serialization ("amd64", "arm64"), defaults to `Amd64`.
Includes `is_amd64()` helper for future `skip_serializing_if` predicates.
Tests verify default, helper method, and serde round-trip behavior.
Part of the Profile-System conversion (wave 1, types-only).
