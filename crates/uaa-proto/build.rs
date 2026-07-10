// file: crates/uaa-proto/build.rs
// version: 1.0.0
// guid: b1fd887b-4926-4509-b33d-ef1134a3b79f
// last-edited: 2026-07-10

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let files = [
        "../../proto/uaa/control/v1/control.proto",
        "../../proto/uaa/enroll/v1/enroll.proto",
        "../../proto/uaa/web/v1/web.proto",
        "../../proto/uaa/pxe/v1/pxe.proto",
        "../../proto/uaa/update/v1/update.proto",
    ];
    let fds = protox::compile(files, ["../../proto"])?;
    tonic_build::configure()
        .build_server(true)
        .build_client(true)
        .compile_fds(fds)?;
    println!("cargo:rerun-if-changed=../../proto");
    Ok(())
}
