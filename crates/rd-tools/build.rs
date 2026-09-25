//! Records the target triple this crate is compiled for.
//!
//! `std::env::consts::{ARCH, OS}` describe the host in a cross-build, which is exactly the
//! case where getting the platform wrong matters most. `TARGET` is set by Cargo for the build
//! script and is the compiler's own answer.
fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let target = std::env::var("TARGET").unwrap_or_else(|_| "unknown".to_owned());
    println!("cargo:rustc-env=RD_TOOLS_TARGET={target}");
}
