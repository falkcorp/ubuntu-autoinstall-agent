<!-- file: changelog.d/fix-reqwest-pin-012.md -->
<!-- version: 1.0.0 -->
<!-- guid: 8a4f2c61-9d37-4e20-b158-6c0e3a7d9f42 -->
<!-- last-edited: 2026-07-22 -->

### Fixed

#### Pin `reqwest` back to 0.12 (0.13 dropped the `rustls-tls` feature)

A dependabot bump moved the workspace `reqwest` to `0.13` while keeping the
`rustls-tls` feature — but `reqwest 0.13` renamed/removed that feature, so every
fresh dependency resolution failed with "package `uaa-control` depends on
`reqwest` with feature `rustls-tls` but `reqwest` does not have that feature",
breaking CI on `main` and every open PR (`Cargo.lock` is not committed, so CI
always resolves fresh). Pinned back to the proven `0.12`; a deliberate `0.13`
migration (new TLS feature names + API review) can happen on its own branch.
