// The Windows version resource both executables carry, `rdownloader.exe` and
// `rdownloader-capture.exe` (RD-1120-12). Included by their `build.rs`, which run it only for a
// Windows target; it writes `rdownloader-version.rc2` into `OUT_DIR` for `windows-resource.rc`.

fn write_version_resource(description: &str, internal_name: &str, original_filename: &str) {
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
            VALUE "FileDescription", "{description}\0"
            VALUE "FileVersion", "{version}\0"
            VALUE "InternalName", "{internal_name}\0"
            VALUE "LegalCopyright", "Copyright (c) Alexander Herling\0"
            VALUE "OriginalFilename", "{original_filename}\0"
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
