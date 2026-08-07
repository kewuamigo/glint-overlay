fn main() -> Result<(), Box<dyn std::error::Error>> {
    let protoc = protoc_bin_vendored::protoc_bin_path()?;
    // SAFETY: build script is single-threaded; PROTOC is only read by prost-build.
    unsafe {
        std::env::set_var("PROTOC", protoc);
    }

    prost_build::Config::new()
        .compile_protos(&["proto/cef_ipc.proto"], &["proto/"])?;

    println!("cargo:rerun-if-changed=proto/cef_ipc.proto");
    Ok(())
}
