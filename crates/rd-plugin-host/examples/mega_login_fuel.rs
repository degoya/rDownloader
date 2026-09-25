//! Prices a MEGA account sign-in inside the sandbox, stage by stage (RD-120-11).
//!
//! `docs/roadmap/jobs/120-11-mega.md` moved this job out of 1.1 on one unmeasured number: what
//! RSA costs a WebAssembly guest against the plugin host's fuel budget. This program measures
//! it, together with the three other stages a sign-in performs, so the answer is a figure
//! rather than an order of magnitude.
//!
//! It is not a test and CI does not run it. `scripts/measure-mega-login-fuel.sh` builds
//! `plugins/mega-login-probe` for `wasm32-unknown-unknown` and runs this against it.
//!
//! The engine is configured exactly as `SandboxEngine` configures the one plugins run in --
//! fuel on, epochs on, NaN canonicalisation on -- because fuel is charged per instruction and
//! a different configuration would be a different price. What is deliberately *not* used is
//! the component model: the probe is a core module with no imports, so what is measured is the
//! computation and not the adapters around a component's call.

use std::{env, path::PathBuf, process::ExitCode};

use anyhow::{Context as _, Result, anyhow};
use wasmtime::{Config, Engine, Instance, Module, Store};

/// What the host gives one invocation, from `rd_plugin_host::DEFAULT_FUEL`.
const DEFAULT_FUEL: u64 = rd_plugin_host::DEFAULT_FUEL;
/// The most a manifest may declare, from `manifest.rs`: twenty times the default.
const MAX_FUEL: u64 = 20 * DEFAULT_FUEL;
/// Enough for the measurement itself. Not a limit anything in the product may use.
const MEASUREMENT_FUEL: u64 = u64::MAX / 2;

/// The stages of a sign-in, in the order `us0`/`us` performs them.
const STAGES: &[(&str, &str)] = &[
    ("probe_noop", "an empty call, for the overhead of calling"),
    (
        "probe_pbkdf2_v2",
        "PBKDF2-HMAC-SHA512, 100 000 rounds (account version 2)",
    ),
    (
        "probe_stringhash_v1",
        "65 536 + 16 384 AES rounds (account version 1)",
    ),
    (
        "probe_unwrap_keys",
        "AES-128-ECB over the master key and the private-key block",
    ),
    (
        "probe_rsa_csid",
        "one RSA-2048 private operation on the session identifier, CRT",
    ),
];

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("mega_login_fuel: {error:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<()> {
    let path = module_path()?;
    let bytes = std::fs::read(&path)
        .with_context(|| format!("read the probe module at {}", path.display()))?;
    let mut config = Config::new();
    config
        .consume_fuel(true)
        .epoch_interruption(true)
        .cranelift_nan_canonicalization(true);
    let engine =
        Engine::new(&config).map_err(|error| anyhow!("create the measuring engine: {error}"))?;
    let module = Module::from_binary(&engine, &bytes)
        .map_err(|error| anyhow!("compile the probe module: {error}"))?;

    println!("module: {} ({} bytes)", path.display(), bytes.len());
    println!("budgets: default {DEFAULT_FUEL}, the largest a manifest may declare {MAX_FUEL}\n");
    println!(
        "{:<22} {:>18} {:>9}  what it is",
        "stage", "fuel", "% of max"
    );

    let mut total = 0_u64;
    for (export, description) in STAGES {
        let used = measure(&engine, &module, export)?;
        if *export != "probe_noop" {
            total = total.saturating_add(used);
        }
        report(export, used, description);
    }
    println!();
    report(
        "version 2 sign-in",
        signin(&engine, &module, "probe_pbkdf2_v2")?,
        "PBKDF2 + unwrap + RSA, what a current account costs",
    );
    report(
        "version 1 sign-in",
        signin(&engine, &module, "probe_stringhash_v1")?,
        "the legacy derivation + unwrap + RSA",
    );
    println!("\nevery stage together: {total}");
    Ok(())
}

/// One stage's cost, with a fresh store so nothing carries over between them.
fn measure(engine: &Engine, module: &Module, export: &str) -> Result<u64> {
    let mut store = Store::new(engine, ());
    store
        .set_fuel(MEASUREMENT_FUEL)
        .map_err(|error| anyhow!("give the measuring store its fuel: {error}"))?;
    store.set_epoch_deadline(u64::MAX);
    let instance = Instance::new(&mut store, module, &[])
        .map_err(|error| anyhow!("instantiate the probe: {error}"))?;
    // Instantiation runs the module's own start-up; charging that to the stage would inflate
    // every one of them by the same constant.
    let before = store
        .get_fuel()
        .map_err(|error| anyhow!("read the fuel before the call: {error}"))?;
    let function = instance
        .get_typed_func::<(), u32>(&mut store, export)
        .map_err(|error| anyhow!("find the export {export}: {error}"))?;
    let answer = function
        .call(&mut store, ())
        .map_err(|error| anyhow!("call {export}: {error}"))?;
    let after = store
        .get_fuel()
        .map_err(|error| anyhow!("read the fuel after the call: {error}"))?;
    // Printed so a stage that silently computed nothing is visible as a repeated checksum.
    eprintln!("{export} -> checksum {answer:#010x}");
    Ok(before.saturating_sub(after))
}

/// A whole sign-in: one derivation, the unwrapping, and the RSA operation.
fn signin(engine: &Engine, module: &Module, derivation: &str) -> Result<u64> {
    let mut total = 0_u64;
    for export in [derivation, "probe_unwrap_keys", "probe_rsa_csid"] {
        total = total.saturating_add(measure(engine, module, export)?);
    }
    Ok(total)
}

fn report(name: &str, fuel: u64, description: &str) {
    let share = (fuel as f64 / MAX_FUEL as f64) * 100.0;
    println!("{name:<22} {fuel:>18} {share:>8.3}%  {description}");
}

/// The module to measure: the first argument, or where a release build leaves it.
fn module_path() -> Result<PathBuf> {
    if let Some(argument) = env::args().nth(1) {
        return Ok(PathBuf::from(argument));
    }
    let target = env::var("CARGO_TARGET_DIR").unwrap_or_else(|_| "target".to_owned());
    Ok(PathBuf::from(target)
        .join("wasm32-unknown-unknown/release")
        .join("mega_login_probe.wasm"))
}
