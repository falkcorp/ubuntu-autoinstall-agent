### Changed

#### Serial console becomes an arch-gated installer default (PS-SERIAL-18)

`configure_serial_console` (grub.d serial-console drop-in for the headless
IPMI-SOL fleet) now runs only when `config.arch == Arch::Amd64`; arm64
targets skip it. This gates on the real serialized `arch` field added by
PS-WIRE-AXES-10 rather than a `#[serde(skip)]` flag, so the behavior survives
`config place` -> installer-reads-serialized-YAML on the target instead of
silently reverting to a default. Because `arch` is
`skip_serializing_if = "Arch::is_amd64"`, every committed amd64 host still
omits the `arch:` key, deserializes back to `Arch::Amd64`, and gets the
serial-console drop-in exactly as before — the placed artifact stays
byte-identical for len-serv/unimatrixone.
