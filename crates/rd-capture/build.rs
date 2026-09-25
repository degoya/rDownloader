use std::{env, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=windows-resource.rc");
    println!("cargo:rerun-if-changed=../../resources/windows-long-path.manifest");
    println!("cargo:rerun-if-changed=../../resources/rdownloader.ico");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        write_version_resource();
        embed_resource::compile("windows-resource.rc", embed_resource::NONE)
            .manifest_required()
            .expect("failed to compile Windows resources");
    }
}

fn write_version_resource() {
    let major = env::var("CARGO_PKG_VERSION_MAJOR").expect("package major version");
    let minor = env::var("CARGO_PKG_VERSION_MINOR").expect("package minor version");
    let patch = env::var("CARGO_PKG_VERSION_PATCH").expect("package patch version");
    let version = env::var("CARGO_PKG_VERSION").expect("package version");
    let resource = format!(
        r#"1 VERSIONINFO
FILEVERSION {major},{minor},{patch},0
PRODUCTVERSION {major},{minor},{patch},0
FILEFLAGSMASK 0x3fL
FILEFLAGS 0x0L
FILEOS 0x40004L
FILETYPE 0x1L
FILESUBTYPE 0x0L
BEGIN
    BLOCK "StringFileInfo"
    BEGIN
        BLOCK "040704B0"
        BEGIN
            VALUE "CompanyName", "Alexander Herling\0"
            VALUE "FileDescription", "rDownloader Click'n'Load Capture Agent\0"
            VALUE "FileVersion", "{version}\0"
            VALUE "InternalName", "rdownloader-capture\0"
            VALUE "LegalCopyright", "Copyright (c) Alexander Herling\0"
            VALUE "OriginalFilename", "rdownloader-capture.exe\0"
            VALUE "ProductName", "rDownloader\0"
            VALUE "ProductVersion", "{version}\0"
        END
    END
    BLOCK "VarFileInfo"
    BEGIN
        VALUE "Translation", 0x0407, 1200
    END
END
"#
    );
    let output = PathBuf::from(env::var_os("OUT_DIR").expect("build output directory"))
        .join("rdownloader-version.rc2");
    std::fs::write(output, resource).expect("write Windows version resource");
}
