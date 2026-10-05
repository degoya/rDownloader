//! Who may name the program an apprise target runs (audit 2026-10-05, S1; RD-1101-11).
//!
//! A target's `config.executable` is a path the service starts. Targets cost `api:config`, and
//! a program path is administration everywhere else (`privileged_change` in the settings), so
//! a save that sets or changes the path needs `api:admin` ([`check_executable`]). That alone
//! would leave a path stored before the rule, or written by a save path that forgot it, free to
//! run; so a save by an administrator also seals the path ([`seal_executable`]) and delivery
//! runs an unsealed one never ([`runnable_executable`]).
//!
//! The seal is a digest over the target's vault reference and the path. The reference is never
//! returned by the API, so a caller who could write `config` verbatim — anybody with
//! `api:config` before this rule — cannot compute a seal, whatever they stored under its key.

use anyhow::Result;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::{NotificationTarget, TargetConfig};

/// The key under a target's `config` that carries the seal. Written by the service on every
/// save; whatever a client sends under it is dropped.
pub const EXECUTABLE_SEAL: &str = "executable_seal";

/// A save that sets or changes `config.executable` without `api:admin`.
#[derive(Debug, PartialEq, Eq)]
pub struct ExecutableNeedsAdmin;

/// Whether a save of `config` may keep its program path sealed, or must be refused.
///
/// `stored` is the target as it is now — its configuration and vault reference — and `None`
/// for a new one. An administrator's path is sealed; a path the caller left as it was keeps
/// the seal it had (and stays unsealed if it had none); any other path is refused. A save
/// without a path needs nothing.
pub fn check_executable(
    config: &Value,
    stored: Option<(&Value, Option<&str>)>,
    holds_admin: bool,
) -> Result<bool, ExecutableNeedsAdmin> {
    let Some(requested) = executable_of(config) else {
        return Ok(false);
    };
    if holds_admin {
        return Ok(true);
    }
    match stored {
        Some((before, secret_ref)) if executable_of(before) == Some(requested) => {
            Ok(sealed(before, secret_ref, requested))
        }
        _ => Err(ExecutableNeedsAdmin),
    }
}

/// Replaces whatever `config` carries under [`EXECUTABLE_SEAL`] with the seal of its path when
/// `approved`, and with nothing otherwise.
///
/// `secret_ref` is the vault reference the target keeps after the save. Without one nothing is
/// sealed: an apprise target without its URL cannot deliver, and a seal over no reference
/// would be one anybody can compute.
pub fn seal_executable(config: &mut Value, approved: bool, secret_ref: Option<&str>) {
    let digest = match (approved, executable_of(config), secret_ref) {
        (true, Some(path), Some(reference)) => Some(seal(reference, path)),
        _ => None,
    };
    if let Some(object) = config.as_object_mut() {
        object.remove(EXECUTABLE_SEAL);
        if let Some(digest) = digest {
            object.insert(EXECUTABLE_SEAL.to_owned(), Value::String(digest));
        }
    }
}

/// The program path delivery may start for `target`: `None` for the lookup, an error for a
/// path no administrator sealed.
pub(crate) fn runnable_executable<'a>(
    target: &NotificationTarget,
    config: &'a TargetConfig,
) -> Result<Option<&'a str>> {
    let Some(path) = config
        .executable
        .as_deref()
        .map(str::trim)
        .filter(|path| !path.is_empty())
    else {
        return Ok(None);
    };
    anyhow::ensure!(
        sealed(&target.config, target.secret_ref.as_deref(), path),
        "the apprise program path was not saved by an administrator; an administrator has to \
         save this target again"
    );
    Ok(Some(path))
}

/// The configured program path, trimmed; `None` when absent, empty or not a string.
fn executable_of(config: &Value) -> Option<&str> {
    config
        .get("executable")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|path| !path.is_empty())
}

fn sealed(config: &Value, secret_ref: Option<&str>, path: &str) -> bool {
    secret_ref.is_some_and(|reference| {
        config.get(EXECUTABLE_SEAL).and_then(Value::as_str) == Some(seal(reference, path).as_str())
    })
}

fn seal(secret_ref: &str, path: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(b"rdownloader:apprise-executable\0");
    digest.update(secret_ref.as_bytes());
    digest.update(b"\0");
    digest.update(path.as_bytes());
    hex::encode(digest.finalize())
}

#[cfg(test)]
#[path = "executable_tests.rs"]
mod tests;
