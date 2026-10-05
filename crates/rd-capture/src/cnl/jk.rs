//! The Click'n'Load key: read straight out of the `jk` field, or evaluated from its script in
//! a bounded sandbox.

use std::{sync::LazyLock, time::Duration};

use anyhow::{Context, Result, bail};
use boa_engine::{Context as JsContext, Source};
use regex::Regex;
use tokio::sync::Semaphore;

const MAX_JK_BYTES: usize = 32 * 1024;
const MAX_JS_INSTRUCTIONS: usize = 100_000;

/// How long the caller waits for a `jk` script to produce a key.
const JK_BUDGET: Duration = Duration::from_millis(250);

/// How many `jk` scripts may be evaluated at the same time.
///
/// Two, because a person answering a Click'n'Load button does it once; anything beyond that is
/// either a retry or a page trying to keep the agent busy. See [`JK_SLOTS`] for why the number
/// has to exist at all.
pub(super) const MAX_CONCURRENT_JK: usize = 2;

/// The slots a `jk` evaluation runs in.
///
/// Boa has no interrupt hook, so a script that is still running cannot be stopped from outside;
/// its instruction budget ([`MAX_JS_INSTRUCTIONS`]) is what ends it. `tokio::time::timeout` only
/// ever ended the *waiting*, which is why this used to be a way to eat the runtime: each call
/// took a thread out of tokio's blocking pool — the same pool `arboard` reads the clipboard on
/// and `notify_rust` raises notifications on — and held it until the budget ran out.
///
/// Two things changed. The evaluation now runs on a thread of its own rather than on the
/// blocking pool, so an over-running script can no longer starve the clipboard or the
/// notifications; and this semaphore caps how many such threads can exist, so a page cannot
/// open one per request. A caller that finds no free slot is refused at once instead of queuing.
pub(super) static JK_SLOTS: Semaphore = Semaphore::const_new(MAX_CONCURRENT_JK);

/// A quoted 32-character hexadecimal literal: the static Click'n'Load key as a script writes it.
///
/// Anchored on the quotes on purpose. The pattern used to be a bare `([0-9a-f]{32})`, which
/// matches *anywhere*: an unrelated 32-digit identifier in the script, or the first half of a
/// 64-digit literal, silently became the key. Decryption then failed with "invalid CNL padding"
/// and nothing said the key had come from the wrong place. With the quotes required, a 64-digit
/// literal no longer matches at all, and more than one match is reported as ambiguous rather
/// than resolved by taking the first.
static QUOTED_STATIC_KEY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"['"]([0-9a-fA-F]{32})['"]"#).expect("static CNL key regex"));

/// The first function declaration in a `jk` script, which is the one that is called.
///
/// `LazyLock` for the same reason as [`QUOTED_STATIC_KEY`] and as
/// `rd_collector::links::URL_PATTERN`: this sits in the request path, and recompiling a regex
/// per request is work a caller gets to ask for for free.
static JK_FUNCTION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)function\s+([A-Za-z_$][A-Za-z0-9_$]*)\s*\(").expect("static CNL jk regex")
});

/// Reads the key straight out of the `jk` field, when it is unambiguously there.
///
/// Three outcomes, and the middle one is the point of this function (RD-109-02):
///
/// - `Ok(Some(key))` — the field is the key itself, or the script contains exactly one quoted
///   32-digit hexadecimal literal.
/// - `Ok(None)` — there is no literal key; the script has to be evaluated.
/// - `Err(..)` — the script contains more than one candidate. That is refused rather than
///   resolved by taking the first, which is what the old unanchored pattern did.
pub(super) fn extract_static_key(source: &str) -> Result<Option<[u8; 16]>> {
    let trimmed = source.trim();
    // The `key` form: the field is the key, nothing else.
    if trimmed.len() == 32 && trimmed.chars().all(|value| value.is_ascii_hexdigit()) {
        return decode_key(trimmed).map(Some);
    }
    let mut candidates: Vec<String> = QUOTED_STATIC_KEY
        .captures_iter(trimmed)
        .filter_map(|capture| capture.get(1))
        .map(|value| value.as_str().to_ascii_lowercase())
        .collect();
    candidates.dedup();
    candidates.sort_unstable();
    candidates.dedup();
    match candidates.len() {
        0 => Ok(None),
        1 => decode_key(&candidates[0]).map(Some),
        count => bail!(
            "CNL jk contains {count} different 128-bit hexadecimal literals; which one is the \
             key cannot be guessed"
        ),
    }
}

pub(super) async fn resolve_key(source: &str) -> Result<[u8; 16]> {
    resolve_key_within(source, JK_BUDGET).await
}

/// The body of [`resolve_key`], with the budget passed in so a test can drive the expiry.
pub(super) async fn resolve_key_within(source: &str, budget: Duration) -> Result<[u8; 16]> {
    if let Some(key) = extract_static_key(source)? {
        return Ok(key);
    }
    if source.len() > MAX_JK_BYTES {
        bail!("CNL jk script exceeds its size limit");
    }
    // `try_acquire`, not `acquire`: a caller that finds every slot taken is told so now rather
    // than joining a queue that a page could make arbitrarily long.
    let permit = JK_SLOTS
        .try_acquire()
        .map_err(|_| anyhow::anyhow!("CNL jk evaluation slots are all busy"))?;
    let source = source.to_owned();
    let (sender, receiver) = tokio::sync::oneshot::channel();
    // A thread of this crate's own, never tokio's blocking pool: an evaluation that outlives
    // its budget then costs one thread that nothing else wanted, instead of one the clipboard
    // and the desktop notifications were going to need.
    std::thread::Builder::new()
        .name("cnl-jk".to_owned())
        .spawn(move || {
            // Released when the thread really ends, not when the caller stops waiting — so the
            // cap counts scripts that are still running, which is the thing worth capping.
            let _permit = permit;
            let _ = sender.send(evaluate_jk(&source));
        })
        .context("start the CNL jk sandbox thread")?;
    let result = tokio::time::timeout(budget, receiver)
        .await
        .context("CNL jk execution timed out")?
        .context("CNL jk sandbox ended without a result")??;
    decode_key(&result)
}

fn evaluate_jk(source: &str) -> Result<String> {
    let function = JK_FUNCTION
        .captures(source)
        .and_then(|captures| captures.get(1))
        .map(|value| value.as_str())
        .context("CNL jk script declares no callable function")?;
    let program = format!("\"use strict\";\n{source}\n{function}();");
    let mut context = JsContext::builder()
        .instructions_remaining(MAX_JS_INSTRUCTIONS)
        .can_block(false)
        .build()
        .map_err(|error| anyhow::anyhow!("create CNL JavaScript sandbox: {error}"))?;
    context
        .runtime_limits_mut()
        .set_loop_iteration_limit(10_000);
    context.runtime_limits_mut().set_recursion_limit(64);
    context.runtime_limits_mut().set_stack_size_limit(1024);
    let value = context
        .eval(Source::from_bytes(program.as_bytes()))
        .map_err(|error| anyhow::anyhow!("evaluate CNL jk script: {error}"))?;
    let value = value
        .to_string(&mut context)
        .map_err(|error| anyhow::anyhow!("convert CNL jk result: {error}"))?
        .to_std_string_escaped();
    if value.len() > 128 {
        bail!("CNL jk result exceeds its size limit");
    }
    Ok(value)
}

fn decode_key(encoded: &str) -> Result<[u8; 16]> {
    let encoded = encoded.trim();
    if encoded.len() != 32
        || !encoded
            .chars()
            .all(|character| character.is_ascii_hexdigit())
    {
        bail!("CNL jk result is not a 128-bit hexadecimal key");
    }
    hex::decode(encoded)?
        .try_into()
        .map_err(|_| anyhow::anyhow!("invalid CNL key length"))
}
