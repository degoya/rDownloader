//! The host's key-derivation primitive: computing over a credential the guest never sees
//! (RD-120-20).
//!
//! `interface auth` states in its own header that a plugin never sees a credential, and the
//! host keeps that promise by substituting `{{username}}` and `{{secret:<reference>}}` on the
//! way *out* of the guest. A guest can therefore *send* a credential; it cannot *compute*
//! with one. MEGA's `us` call wants neither: it wants the second half of
//! PBKDF2(password, salt, 100 000), and the first half unwraps the account's master key.
//!
//! So the host computes and the guest only names. This module holds three separable things:
//!
//! * the **shape rules** — what a chain of steps may look like, checked before anything is
//!   computed and before anything is charged;
//! * the **price** — what a chain costs against the caller's fuel budget, at the rate the
//!   same computation was measured to cost inside the sandbox (RD-120-11);
//! * the **arithmetic** — PBKDF2-HMAC-SHA512, AES-128-ECB and a window, over a value that is
//!   overwritten on the way out.
//!
//! Everything here is pure: no vault, no registry, no store. Which credential a reference
//! stands for is `native::host`'s question, and whether the plugin may ask at all is
//! `native::granted`'s. `docs/adr/0020-the-host-computes-over-the-secret.md` records why the
//! bytes that come back do not reconstruct the credential.

use aes::{
    Aes128,
    cipher::{BlockDecrypt, KeyInit},
};
use hmac::{Hmac, Mac};
use rd_core::{Failure, FailureKind};
use rd_plugin_api::DerivationStep;
use sha2::Sha512;
use zeroize::Zeroizing;

mod origin;
pub use origin::{SESSION_KEY_BYTES, SecretOrigin};

/// Fewest PBKDF2 iterations the host will perform.
///
/// The number is MEGA's own, and it is a floor rather than a default because of what a
/// cheaper one would make this interface. A derivation the host runs for a handful of
/// iterations is an oracle: a guest feeds candidate passwords past it for almost nothing and
/// reads the answer. At this floor a single candidate costs what a candidate costs anywhere,
/// which is the property the whole primitive rests on.
pub const MIN_PBKDF2_ROUNDS: u32 = 100_000;
/// Most bytes one PBKDF2 step may produce. One SHA-512 block; MEGA asks for half of it.
pub const MAX_DERIVED_BYTES: u32 = 64;
/// Most bytes one AES step may unwrap. MEGA's private-key block is 656.
pub const MAX_WRAPPED_BYTES: usize = 4096;
/// Most steps one chain may hold. MEGA's longest is four.
pub const MAX_STEPS: usize = 8;
/// One AES block.
const BLOCK: usize = 16;
/// One SHA-512 block, which is also PBKDF2-HMAC-SHA512's output width.
const HASH_BLOCK: u32 = 64;

/// Fuel one PBKDF2 iteration costs, per block of output.
///
/// Measured, not chosen. `plugins/mega-login-probe` computed PBKDF2-HMAC-SHA512 with 100 000
/// iterations and 32 bytes of output inside the sandbox on 2026-09-22 and it cost
/// 3 338 300 549 fuel (RD-120-11) — 33 383.005 per iteration for the one hash block that
/// covers 32 bytes. Charging the measured guest price is the whole rule: see
/// [`fuel_cost`].
pub const PBKDF2_FUEL_PER_ROUND: u64 = 33_383;
/// Fuel one AES-128-ECB block costs, key schedule amortised.
///
/// From the same measurement: unwrapping the master key and the private-key block — 42
/// blocks under two key schedules — cost 342 965 fuel, or 8 165.83 per block. Rounded up, so
/// the host never undercharges.
pub const AES_ECB_FUEL_PER_BLOCK: u64 = 8_166;
/// Fuel a window costs, per byte kept. A guest would have copied them.
pub const TAKE_FUEL_PER_BYTE: u64 = 1;
/// Fuel a call costs before any step runs. The measured cost of an empty guest call.
pub const CALL_FUEL: u64 = 2;

fn refuse(code: &'static str, message: impl Into<String>) -> Failure {
    Failure::coded(FailureKind::Permanent, code, message.into())
}

/// Checks a chain against the credential it will run over, before anything is computed or
/// the vault is touched.
///
/// The rule that matters is the first one, and it depends on where the credential came from
/// ([`SecretOrigin`], RD-120-30). **Over something a person typed, a chain begins with
/// `pbkdf2-hmac-sha512`** at or above [`MIN_PBKDF2_ROUNDS`]: the first thing that happens to a
/// guessable value is the one-way thing. **Over the key a sign-in left, a chain begins with
/// `aes-ecb-decrypt`**, keyed by the whole of that key. **A window first is refused either
/// way**, because it would answer with the credential. The argument for each half is in
/// `docs/adr/0020-the-host-computes-over-the-secret.md`, addendum of RD-120-30.
///
/// The origin is the host's to say and never the guest's: it is decided from where the
/// value is read, not from anything the guest sent.
pub fn validate(steps: &[DerivationStep], origin: SecretOrigin) -> Result<(), Failure> {
    let first = steps.first().ok_or_else(|| {
        refuse(
            "plugin.key_derivation_steps_invalid",
            "A derivation with no steps would be asking for the credential itself",
        )
    })?;
    origin.admits_first(first)?;
    validate_shape(steps)
}

/// The rules that hold whatever the credential is: bounds, and never a window first.
///
/// Checked by the linked `derive` before the chain is priced, when the host does not yet know
/// which credential the handle stands for. [`validate`] repeats it with the origin once it
/// does, before the vault is touched.
pub fn validate_shape(steps: &[DerivationStep]) -> Result<(), Failure> {
    if steps.is_empty() {
        return Err(refuse(
            "plugin.key_derivation_steps_invalid",
            "A derivation with no steps would be asking for the credential itself",
        ));
    }
    if steps.len() > MAX_STEPS {
        return Err(refuse(
            "plugin.key_derivation_steps_invalid",
            format!("A derivation may hold at most {MAX_STEPS} steps"),
        ));
    }
    if matches!(steps[0], DerivationStep::Take { .. }) {
        return Err(refuse(
            "plugin.key_derivation_needs_one_way",
            "A derivation may not begin with a window onto the credential itself",
        ));
    }
    for step in steps {
        match step {
            DerivationStep::Pbkdf2HmacSha512 { rounds, length, .. } => {
                if *rounds < MIN_PBKDF2_ROUNDS {
                    return Err(refuse(
                        "plugin.key_derivation_rounds_too_low",
                        format!("PBKDF2 needs at least {MIN_PBKDF2_ROUNDS} rounds here"),
                    ));
                }
                if *length == 0 || *length > MAX_DERIVED_BYTES {
                    return Err(refuse(
                        "plugin.key_derivation_steps_invalid",
                        format!("A PBKDF2 step produces 1 to {MAX_DERIVED_BYTES} bytes"),
                    ));
                }
            }
            DerivationStep::Aes128EcbDecrypt(data) => {
                if data.is_empty()
                    || data.len() > MAX_WRAPPED_BYTES
                    || !data.len().is_multiple_of(BLOCK)
                {
                    return Err(refuse(
                        "plugin.key_derivation_steps_invalid",
                        format!(
                            "An aes-ecb-decrypt step takes 1 to {} whole blocks",
                            MAX_WRAPPED_BYTES / BLOCK
                        ),
                    ));
                }
            }
            DerivationStep::Take { length, .. } => {
                if *length == 0 || *length > MAX_WRAPPED_BYTES as u32 {
                    return Err(refuse(
                        "plugin.key_derivation_steps_invalid",
                        "A window keeps between one byte and the running value",
                    ));
                }
            }
        }
    }
    Ok(())
}

/// What a chain costs against the caller's fuel budget.
///
/// **The rate is the measured guest price, and that is the argument.** The cap exists to
/// bound how much computation a plugin may cause; a primitive that charged less than the
/// guest's own arithmetic would be a discount, and the way around the cap the job warned
/// about. One that charged more would be unusable and would push the credential back into
/// the sandbox, which is the thing being prevented. Charging exactly the guest price makes
/// the primitive **fuel-neutral**: computing here costs a plugin what computing there would
/// have cost it, so the only thing that changes is where the credential lives.
///
/// The wall-clock consequence — the host does natively in milliseconds what the guest would
/// have spent seconds on — is bounded by the other cap and not by this one: host time counts
/// against the invocation's execution deadline exactly as guest time does.
///
/// Call [`validate`] first; the bounds it enforces are what keeps this arithmetic from
/// overflowing.
#[must_use]
pub fn fuel_cost(steps: &[DerivationStep]) -> u64 {
    let mut cost = CALL_FUEL;
    for step in steps {
        cost = cost.saturating_add(match step {
            DerivationStep::Pbkdf2HmacSha512 { rounds, length, .. } => {
                let blocks = u64::from(length.div_ceil(HASH_BLOCK).max(1));
                u64::from(*rounds)
                    .saturating_mul(blocks)
                    .saturating_mul(PBKDF2_FUEL_PER_ROUND)
            }
            DerivationStep::Aes128EcbDecrypt(data) => {
                (data.len() / BLOCK) as u64 * AES_ECB_FUEL_PER_BLOCK
            }
            DerivationStep::Take { length, .. } => u64::from(*length) * TAKE_FUEL_PER_BYTE,
        });
    }
    cost
}

/// Runs a validated chain over `secret`.
///
/// The running value is held in [`Zeroizing`] throughout, so neither the credential nor any
/// intermediate survives the call in the host's own memory. What is returned is the last
/// step's output and nothing else.
pub fn run(secret: &[u8], steps: &[DerivationStep]) -> Result<Vec<u8>, Failure> {
    let mut current = Zeroizing::new(secret.to_vec());
    for step in steps {
        current = match step {
            DerivationStep::Pbkdf2HmacSha512 {
                salt,
                rounds,
                length,
            } => Zeroizing::new(pbkdf2_hmac_sha512(
                &current,
                salt,
                *rounds,
                *length as usize,
            )),
            DerivationStep::Aes128EcbDecrypt(data) => {
                if current.len() < BLOCK {
                    return Err(refuse(
                        "plugin.key_derivation_steps_invalid",
                        "An aes-ecb-decrypt step needs sixteen bytes of key before it",
                    ));
                }
                let mut key = [0_u8; BLOCK];
                key.copy_from_slice(&current[..BLOCK]);
                let out = decrypt_ecb(&key, data);
                key.fill(0);
                Zeroizing::new(out)
            }
            DerivationStep::Take { offset, length } => {
                let start = *offset as usize;
                let end = start.saturating_add(*length as usize);
                if end > current.len() {
                    return Err(refuse(
                        "plugin.key_derivation_steps_invalid",
                        "A window reaches past the value it was applied to",
                    ));
                }
                Zeroizing::new(current[start..end].to_vec())
            }
        };
    }
    Ok(current.to_vec())
}

/// PBKDF2-HMAC-SHA512 for a derived key of at most one hash block.
///
/// Written out rather than pulled from a crate, because the measurement this module's price
/// comes from computed it exactly this way (`plugins/mega-login-probe`) — a different
/// implementation would be a different number.
fn pbkdf2_hmac_sha512(password: &[u8], salt: &[u8], rounds: u32, length: usize) -> Vec<u8> {
    let round = |data: &[u8]| -> Zeroizing<Vec<u8>> {
        let mut mac = <Hmac<Sha512> as Mac>::new_from_slice(password)
            .expect("HMAC takes a key of any length");
        mac.update(data);
        Zeroizing::new(mac.finalize().into_bytes().to_vec())
    };
    let mut block = salt.to_vec();
    block.extend_from_slice(&1_u32.to_be_bytes());
    let mut current = round(&block);
    let mut accumulated = current.clone();
    for _ in 1..rounds {
        current = round(&current);
        for (into, from) in accumulated.iter_mut().zip(current.iter()) {
            *into ^= *from;
        }
    }
    accumulated[..length.min(accumulated.len())].to_vec()
}

/// AES-128-ECB over whole blocks.
fn decrypt_ecb(key: &[u8; BLOCK], data: &[u8]) -> Vec<u8> {
    let cipher = Aes128::new(key.into());
    let mut out = data.to_vec();
    let (blocks, _) = out.as_chunks_mut::<BLOCK>();
    for block in blocks {
        cipher.decrypt_block(block.into());
    }
    out
}

/// The interface name the guest imports, version and all.
const INTERFACE: &str = "rdownloader:plugin/key-derivation@0.9.0";

/// The guest's `secret-handle`.
#[derive(wasmtime::component::ComponentType, wasmtime::component::Lift)]
#[component(record)]
pub struct WitSecretHandle {
    reference: String,
}

/// The guest's `pbkdf2` parameters.
#[derive(wasmtime::component::ComponentType, wasmtime::component::Lift)]
#[component(record)]
pub struct WitPbkdf2 {
    salt: Vec<u8>,
    rounds: u32,
    length: u32,
}

/// The guest's `span`.
#[derive(wasmtime::component::ComponentType, wasmtime::component::Lift)]
#[component(record)]
pub struct WitSpan {
    offset: u32,
    length: u32,
}

/// The guest's `step`.
#[derive(wasmtime::component::ComponentType, wasmtime::component::Lift)]
#[component(variant)]
pub enum WitStep {
    #[component(name = "pbkdf2-hmac-sha512")]
    Pbkdf2HmacSha512(WitPbkdf2),
    #[component(name = "aes-ecb-decrypt")]
    Aes128EcbDecrypt(Vec<u8>),
    #[component(name = "take")]
    Take(WitSpan),
}

impl From<WitStep> for DerivationStep {
    fn from(value: WitStep) -> Self {
        match value {
            WitStep::Pbkdf2HmacSha512(params) => Self::Pbkdf2HmacSha512 {
                salt: params.salt,
                rounds: params.rounds,
                length: params.length,
            },
            WitStep::Aes128EcbDecrypt(data) => Self::Aes128EcbDecrypt(data),
            WitStep::Take(span) => Self::Take {
                offset: span.offset,
                length: span.length,
            },
        }
    }
}

/// Links `derive` into one plugin's linker.
///
/// Hand-wired rather than taken from `bindgen!`, and for one reason: a generated host
/// function is handed the store's *data*, and fuel lives on the store itself.
/// [`wasmtime::component::LinkerInstance::func_wrap_async`] hands over a
/// [`wasmtime::StoreContextMut`], which is what makes the charge below possible at all —
/// before the work is done, inside the same call, against the one budget the guest is
/// running on. An accounting that happened after the call would be a bill for work the host
/// had already performed, which is not a cap.
pub(crate) fn add_to_linker(
    linker: &mut wasmtime::component::Linker<crate::runtime::PluginStoreState>,
) -> anyhow::Result<()> {
    linker
        .instance(INTERFACE)?
        .func_wrap_async::<(WitSecretHandle, Vec<WitStep>), (Result<Vec<u8>, WitFailure>,), _>(
            "derive",
            |mut store, (handle, steps)| {
                Box::new(async move { Ok((derive(&mut store, handle, steps).await,)) })
            },
        )?;
    Ok(())
}

type WitFailure = crate::component::rdownloader::plugin::types::Failure;

fn wit_refusal(code: &'static str, message: &str) -> WitFailure {
    crate::component::to_wit_failure(Failure::coded(
        FailureKind::Permanent,
        code,
        message.to_owned(),
    ))
}

async fn derive(
    store: &mut wasmtime::StoreContextMut<'_, crate::runtime::PluginStoreState>,
    handle: WitSecretHandle,
    steps: Vec<WitStep>,
) -> Result<Vec<u8>, WitFailure> {
    let steps: Vec<DerivationStep> = steps.into_iter().map(DerivationStep::from).collect();
    // Shape first, then price, then the credential. A chain whose shape the host would refuse
    // costs the guest nothing but the call. Which first step the credential admits depends on
    // where it came from, which only the host behind this call knows; a chain that fails
    // *that* rule has been paid for and is refused before the vault is touched (RD-120-30).
    validate_shape(&steps).map_err(crate::component::to_wit_failure)?;
    let cost = fuel_cost(&steps);
    let remaining = store.get_fuel().map_err(|_| {
        wit_refusal(
            "plugin.key_derivation_budget",
            "This invocation has no fuel",
        )
    })?;
    if cost > remaining {
        // What the guest would have spent trying, spent. Asking the host for more
        // computation than the budget holds is not a free probe.
        let _ = store.set_fuel(0);
        return Err(wit_refusal(
            "plugin.key_derivation_budget",
            "This derivation costs more fuel than the invocation has left",
        ));
    }
    store.set_fuel(remaining - cost).map_err(|_| {
        wit_refusal(
            "plugin.key_derivation_budget",
            "This invocation has no fuel",
        )
    })?;
    let state = store.data();
    let Some(host) = state.host() else {
        return Err(crate::component::to_wit_failure(Failure::coded(
            FailureKind::Unsupported,
            "plugin.key_derivation_unsupported",
            "Deriving from a credential is not supported by this host",
        )));
    };
    let identity = state.identity().clone();
    host.derive_from_secret(&identity, &handle.reference, &steps)
        .await
        .map_err(crate::component::to_wit_failure)
}

#[cfg(test)]
mod tests;
