//! The one piece of arithmetic that stays in the guest: RSA over the session identifier.
//!
//! MEGA's `us` answers with `csid`, the session identifier encrypted to the account's own
//! public key, and with `privk`, the matching private key wrapped under the master key. The
//! host unwraps `privk` (it is a key-derivation stage over the credential), and what is left
//! is a private operation over ciphertext -- nothing derived from the password, and nothing a
//! host primitive is needed for. It was measured at 189 444 831 fuel with the Chinese
//! remainder theorem, which is under a tenth of a default budget
//! (`docs/roadmap/jobs/120-11-mega.md`, Phase 3).
//!
//! `privk` is four MPIs in MEGA's own framing: a two-byte big-endian *bit* length, then that
//! many bits rounded up to whole bytes, four times over, in the order `p`, `q`, `d`, `u`.
//! Trailing padding after the fourth is the AES block padding and is ignored.

use num_bigint::BigUint;

/// Longest private-key block this will read. A 2048-bit key's four MPIs come to 656 bytes.
const MAX_BLOCK: usize = 4096;
/// Smallest modulus half this accepts, in bytes. Below it the answer is not an RSA key.
const MIN_PRIME_BYTES: usize = 32;

/// The four values a MEGA private-key block carries.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PrivateKey {
    p: BigUint,
    q: BigUint,
    d: BigUint,
    /// `p^-1 mod q`, which MEGA stores rather than recomputing.
    u: BigUint,
}

impl PrivateKey {
    /// Reads the four MPIs. `None` for anything that is not four well-framed numbers.
    #[must_use]
    pub fn parse(block: &[u8]) -> Option<Self> {
        if block.len() > MAX_BLOCK {
            return None;
        }
        let mut rest = block;
        let mut values = Vec::with_capacity(4);
        for _ in 0..4 {
            let (value, remainder) = read_mpi(rest)?;
            values.push(value);
            rest = remainder;
        }
        let u = values.pop()?;
        let d = values.pop()?;
        let q = values.pop()?;
        let p = values.pop()?;
        Some(Self { p, q, d, u })
    }

    /// `m = c^d mod n`, by the two half-width exponentiations every implementation uses.
    ///
    /// `None` when the ciphertext is not a number smaller than the modulus, or when the key
    /// is too small to be one.
    #[must_use]
    pub fn decrypt(&self, ciphertext: &[u8]) -> Option<Vec<u8>> {
        if ciphertext.is_empty()
            || ciphertext.len() > MAX_BLOCK
            || self.p.to_bytes_be().len() < MIN_PRIME_BYTES
            || self.q.to_bytes_be().len() < MIN_PRIME_BYTES
        {
            return None;
        }
        let one = BigUint::from(1_u32);
        if self.p <= one || self.q <= one {
            return None;
        }
        let c = BigUint::from_bytes_be(ciphertext);
        let exponent_p = &self.d % (&self.p - &one);
        let exponent_q = &self.d % (&self.q - &one);
        let message_p = c.modpow(&exponent_p, &self.p);
        let message_q = c.modpow(&exponent_q, &self.q);
        // `(m_q - m_p) * u mod q`, with the subtraction kept non-negative the long way round.
        let difference = (&message_q + &self.q - (&message_p % &self.q)) % &self.q;
        let helper = (difference * &self.u) % &self.q;
        Some((message_p + helper * &self.p).to_bytes_be())
    }
}

/// One MPI: two bytes of bit length, then the bytes.
fn read_mpi(bytes: &[u8]) -> Option<(BigUint, &[u8])> {
    let (header, rest) = bytes.split_at_checked(2)?;
    let bits = usize::from(u16::from_be_bytes([header[0], header[1]]));
    let length = bits.div_ceil(8);
    if length == 0 || length > MAX_BLOCK {
        return None;
    }
    let (value, rest) = rest.split_at_checked(length)?;
    Some((BigUint::from_bytes_be(value), rest))
}

#[cfg(test)]
#[path = "rsa_tests.rs"]
mod rsa_tests;
