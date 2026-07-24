### Added

#### Profile system: network authoring sub-struct + addressing enum

New `Addressing` enum (DHCP vs. static IP with address and gateway) and
`NetworkConfigPartial` struct for type-safe network configuration in host and
group profiles. The `Addressing` enum replaces the magic `network_address == "dhcp"`
string sentinel, providing self-documenting authoring-time types that serialize
to tagged JSON (`{"type":"dhcp"}` or `{"type":"static",…}`). Maps to flat wire
fields via PS-LOWER-12.
