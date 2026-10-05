#!/usr/bin/env bash
# shellcheck shell=bash
#
# The two whole-workspace lints of scripts/check.sh, kept here so check.sh stays readable: clippy
# for Linux (`--clippy-all`, and the Linux half of `--gate`) and `cargo xwin clippy` for Windows
# (`--windows`, and the Windows half of `--gate`, RD-140-23). Every crate, all targets and all
# features, which is what CI's `rust` job lints, at -j 2: the whole workspace at once has taken
# WSL into swap at higher parallelism.
#
# Both with `--keep-going` (RD-1100-13): a lint that stops at the first crate that does not
# compile shows that crate's errors and nothing else, and the 1.9.1 integration needed four fix
# rounds for 24 files that way. With it, cargo builds every crate it still can and one round
# shows every error.
#
# Expects the lock taken and the working directory at the checkout root. Each returns cargo's
# exit status; the caller decides what a red one means.

rd_lint_linux() {
    echo "==> cargo clippy over the workspace, all targets and features (--keep-going, -j 2)"
    CARGO_BUILD_JOBS=2 cargo clippy --workspace --all-targets --all-features --keep-going -j 2 \
        -- -D warnings
}

# Every crate by name: `--` arguments reach the selected packages only. All targets and features,
# because tests compile differently on Windows (RD-120-67).
rd_lint_windows() {
    local manifest
    local -a args=()
    if ! cargo xwin --version > /dev/null 2>&1; then
        echo "cargo xwin is not installed (cargo install cargo-xwin --version 0.23.1 --locked)" >&2
        return 1
    fi
    for manifest in crates/*/Cargo.toml; do args+=(-p "$(basename "$(dirname "$manifest")")"); done
    echo "==> cargo xwin clippy for x86_64-pc-windows-msvc over $(( ${#args[@]} / 2 )) crates (--keep-going, -j 2)"
    CARGO_BUILD_JOBS=2 cargo xwin clippy --target x86_64-pc-windows-msvc -j 2 "${args[@]}" \
        --all-targets --all-features --keep-going -- -D warnings
}
