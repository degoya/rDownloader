//! Where a credential came from, and the one rule of a derivation that depends on it
//! (RD-120-30).
//!
//! RD-120-20 opened every chain with PBKDF2, and for a reason that holds exactly as long as
//! the credential is something a person chose: a password has guessed entropy, so the first
//! thing that happens to it has to be the expensive one-way step, or the host is a cheap
//! oracle a dictionary runs through. The key a sign-in leaves behind is not that. MEGA's
//! master key is sixteen random bytes MEGA's own client made; nobody guesses it, and putting
//! PBKDF2 in front of it protects nothing while making it useless as a key.
//!
//! So the rule is replaced rather than removed. It still says what the first step must be;
//! it now says it per origin, and the origin is the host's to know:
//!
//! * [`SecretOrigin::Person`] -- the accounts form's value, read from `accounts.secret_ref`.
//!   First step `pbkdf2-hmac-sha512`, unchanged.
//! * [`SecretOrigin::SignIn`] -- the key material a sign-in stored beside its session token,
//!   read from `auth_flows.key_ref`, which nothing but the host's `store-token` path writes.
//!   First step `aes-ecb-decrypt`, keyed by all sixteen bytes of it.
//!
//! A window first is refused for both: over a typed secret it *is* the secret, and over a
//! stored key it is a sliding window that recovers the key a byte at a time.

use rd_core::{Failure, FailureKind};
use rd_plugin_api::DerivationStep;

/// How long the key material a sign-in may leave is: one AES-128 key, exactly.
///
/// Exact rather than "at least", because the AES step keys itself with the first sixteen
/// bytes of the running value. Were the stored key longer, the bytes past sixteen would be
/// key material no chain can use -- and a window, the only step that could reach them, is
/// what this module refuses first.
pub const SESSION_KEY_BYTES: usize = 16;

/// Where the value behind a secret handle came from.
///
/// Decided by the host from **where it reads the value**, never from anything the guest sent:
/// the WIT `secret-handle` is a name and nothing else, and a plugin has no way to say which
/// kind of credential it names. See `NativeHost::derive_from_secret`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SecretOrigin {
    /// Typed by a person into the accounts form -- a password, an API key. Its entropy is
    /// whatever a person chose, so guessing it is the threat.
    Person,
    /// Sixteen bytes of key material a sign-in stored beside its session token. Not typed,
    /// not chosen by a person, and held where no form and no other interface writes.
    SignIn,
}

impl SecretOrigin {
    /// Whether a chain over a credential of this origin may begin with `first`.
    ///
    /// # Errors
    ///
    /// `plugin.key_derivation_needs_one_way` for a typed credential whose chain does not open
    /// with PBKDF2, and `plugin.key_derivation_needs_key_step` for sign-in key material whose
    /// chain does not open with AES.
    pub fn admits_first(self, first: &DerivationStep) -> Result<(), Failure> {
        match (self, first) {
            (Self::Person, DerivationStep::Pbkdf2HmacSha512 { .. })
            | (Self::SignIn, DerivationStep::Aes128EcbDecrypt(_)) => Ok(()),
            (Self::Person, _) => Err(Failure::coded(
                FailureKind::Permanent,
                "plugin.key_derivation_needs_one_way",
                "A derivation over a typed credential has to begin with pbkdf2-hmac-sha512"
                    .to_owned(),
            )),
            (Self::SignIn, _) => Err(Failure::coded(
                FailureKind::Permanent,
                "plugin.key_derivation_needs_key_step",
                "A derivation over sign-in key material has to begin with aes-ecb-decrypt"
                    .to_owned(),
            )),
        }
    }
}

#[cfg(test)]
#[path = "origin_tests.rs"]
mod origin_tests;
