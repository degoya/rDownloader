use std::{env, path::PathBuf};

include!("../../resources/build/windows_version.rs");

fn main() {
    println!("cargo:rerun-if-changed=windows-resource.rc");
    println!("cargo:rerun-if-changed=../../resources/windows-long-path.manifest");
    println!("cargo:rerun-if-changed=../../resources/rdownloader.ico");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        write_version_resource(
            "rDownloader Click'n'Load Capture Agent",
            "rdownloader-capture",
            "rdownloader-capture.exe",
        );
        embed_resource::compile("windows-resource.rc", embed_resource::NONE)
            .manifest_required()
            .expect("failed to compile Windows resources");
    }
}
