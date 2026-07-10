// file: crates/uaa-proto/src/lib.rs
// version: 1.0.0
// guid: 691b3989-e011-4a23-986c-f2886a9707f3
// last-edited: 2026-07-10

//! Generated gRPC/protobuf types for the uaa constellation.
//!
//! Each module below re-exports the code protox + tonic-build generate from
//! `proto/uaa/**` at build time (see `build.rs`). Generated code is never
//! committed; it lives under `OUT_DIR`.

pub mod control {
    pub mod v1 {
        tonic::include_proto!("uaa.control.v1");
    }
}

pub mod enroll {
    pub mod v1 {
        tonic::include_proto!("uaa.enroll.v1");
    }
}

pub mod web {
    pub mod v1 {
        tonic::include_proto!("uaa.web.v1");
    }
}

pub mod pxe {
    pub mod v1 {
        tonic::include_proto!("uaa.pxe.v1");
    }
}

pub mod update {
    pub mod v1 {
        tonic::include_proto!("uaa.update.v1");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prost::Message;

    #[test]
    fn test_machine_roundtrip() {
        let machine = control::v1::Machine {
            mac: "aa:bb:cc:dd:ee:ff".to_string(),
            boot_target: "install".to_string(),
            consistent: true,
            ..Default::default()
        };

        let mut buf = Vec::new();
        machine.encode(&mut buf).expect("encode Machine");
        let decoded = control::v1::Machine::decode(buf.as_slice()).expect("decode Machine");

        assert_eq!(machine, decoded);
    }

    #[test]
    fn test_manifest_min_version_field() {
        let manifest = update::v1::Manifest {
            binaries: vec![update::v1::BinaryEntry {
                name: "uaa".to_string(),
                version: "1.2.3".to_string(),
                target: "x86_64-unknown-linux-musl".to_string(),
                sha256: "deadbeef".to_string(),
                sig: "sig".to_string(),
                url: "https://example.invalid/uaa".to_string(),
            }],
            min_version: "1.2.3".to_string(),
        };

        let mut buf = Vec::new();
        manifest.encode(&mut buf).expect("encode Manifest");
        let decoded = update::v1::Manifest::decode(buf.as_slice()).expect("decode Manifest");

        assert_eq!(decoded.min_version, "1.2.3");
        assert_eq!(manifest, decoded);
    }
}
