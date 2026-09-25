use std::{
    env,
    path::{Path, PathBuf},
    process::Command,
};

fn main() {
    println!("cargo:rerun-if-changed=windows-resource.rc");
    println!("cargo:rerun-if-changed=../../resources/windows-long-path.manifest");
    stamp_build();
    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        write_version_resource("rDownloader", "rDownloader", "rdownloader.exe");
        embed_resource::compile("windows-resource.rc", embed_resource::NONE)
            .manifest_required()
            .expect("failed to compile Windows resources");
    }
}

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

/// Compiles the commit and the build time into the binary, for the About page (RD-130-12).
///
/// A package build exports both through `rd_build_stamp` in `scripts/lib/version-file.sh` — the
/// function that also writes them into VERSION.txt — so the package and the running service
/// cannot disagree, and whatever form the script gives the commit is taken verbatim. Only a
/// development build, where nobody exported them, works them out here, by the script's rule:
/// eight characters of the commit, `-dirty` when the tree differs from it, `unknown` without git.
fn stamp_build() {
    println!("cargo:rerun-if-env-changed=RD_BUILD_COMMIT");
    println!("cargo:rerun-if-env-changed=RD_BUILD_TIME");
    let commit = exported("RD_BUILD_COMMIT").unwrap_or_else(git_commit);
    let built = exported("RD_BUILD_TIME")
        .unwrap_or_else(|| chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string());
    println!("cargo:rustc-env=RD_BUILD_COMMIT={commit}");
    println!("cargo:rustc-env=RD_BUILD_TIME={built}");
}

fn exported(name: &str) -> Option<String> {
    env::var(name).ok().filter(|value| !value.trim().is_empty())
}

fn git(arguments: &[&str]) -> Option<String> {
    let output = Command::new("git").args(arguments).output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn git_commit() -> String {
    // Without these the script would run once and a development binary would name the commit
    // it was first built at for good. The index stands in for the working tree: `-dirty` is as
    // current as the last `git add`, which is as close as a build script can watch cheaply.
    let mut watched = vec![
        "HEAD".to_owned(),
        "index".to_owned(),
        "packed-refs".to_owned(),
    ];
    watched.extend(git(&["symbolic-ref", "-q", "HEAD"]));
    for name in watched {
        if let Some(path) = git(&["rev-parse", "--git-path", &name])
            && Path::new(&path).exists()
        {
            println!("cargo:rerun-if-changed={path}");
        }
    }
    let Some(commit) = git(&["rev-parse", "--short=8", "HEAD"]) else {
        return "unknown".to_owned();
    };
    // Exit code 1 is "differs"; anything else is git failing, which is not a dirty tree.
    let dirty = Command::new("git")
        .args(["diff", "--quiet", "HEAD", "--"])
        .status()
        .is_ok_and(|status| status.code() == Some(1));
    if dirty {
        format!("{commit}-dirty")
    } else {
        commit
    }
}
