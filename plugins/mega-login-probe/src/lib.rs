//! What a MEGA account sign-in would cost inside the WebAssembly sandbox (RD-120-11).
//!
//! **This is a measurement instrument, not a plugin.** It has no `manifest.toml`, it is never
//! packaged, signed or installed, and `scripts/build-plugins.sh` skips it for exactly that
//! reason -- the same standing as `plugins/guest`. Its only caller is
//! `cargo run -p rd-plugin-host --example mega_login_fuel`, driven by
//! `scripts/measure-mega-login-fuel.sh`.
//!
//! ## Why it exists
//!
//! `docs/roadmap/jobs/120-11-mega.md`, section 7, arrived at "roughly 200 Wasm instructions
//! per AES block" and said plainly that the number was arithmetic rather than a measurement.
//! That estimate is what moved this job out of 1.1. The account sign-in is the part it was
//! never taken for at all: MEGA's `us0`/`us` pair is not an AES loop over a file but four
//! separate pieces of work, and one of them is an RSA private operation -- the single thing
//! nobody had priced.
//!
//! The four exports below are those pieces, in the order a sign-in performs them. Each is the
//! real computation over real-shaped input, and each returns a checksum of its own output so
//! that no part of it can be optimised away as dead.
//!
//! ## The key material
//!
//! `P`, `Q`, `D`, `U` and `CSID` below are a throwaway RSA-2048 key generated on 2026-09-22
//! for this file alone. It protects nothing, it was never a MEGA account's, and it is here
//! because an RSA measurement needs a modulus of the right size and a ciphertext that really
//! decrypts. No credential of any person or provider is in this crate, and none ever may be.

// The four exports are the measurement's surface; `extern "C"` with a stable name is the only
// way a core module offers one. Nothing else here is unsafe.
#![allow(unsafe_code)]

use aes::{
    Aes128,
    cipher::{BlockDecrypt, BlockEncrypt, KeyInit},
};
use hmac::{Hmac, Mac};
use num_bigint::BigUint;
use sha2::Sha512;

/// Rounds MEGA's account version 2 derives with. Fixed by the provider, not by us.
const PBKDF2_ROUNDS: u32 = 100_000;
/// Rounds the legacy version 1 key preparation runs.
const V1_PREPARE_ROUNDS: u32 = 65_536;
/// Rounds the legacy version 1 e-mail hash runs.
const V1_STRINGHASH_ROUNDS: u32 = 16_384;

/// A password of the length people actually pick. Not anybody's.
const PASSWORD: &[u8] = b"correct horse battery";
/// A 32-byte account salt, the shape `us0` answers with for a version 2 account.
const SALT: &[u8] = b"mega-login-probe-salt-0123456789";

/// The encrypted master key, as `us` answers with it: one AES block.
const WRAPPED_MASTER_KEY: [u8; 16] = [
    0x3c, 0x9a, 0x11, 0x70, 0xd2, 0x4e, 0x8b, 0x05, 0xe7, 0x61, 0x2f, 0xc8, 0x94, 0x0d, 0xa3, 0x56,
];
/// Bytes of the encrypted private key block `us` answers with. Four MPIs of a 2048-bit key,
/// padded to an AES block boundary: 656 bytes, or 41 blocks.
const WRAPPED_PRIVATE_KEY_BYTES: usize = 656;

/// The throwaway key's first prime.
const P: &str = "fe545a7dbaf102f8a264d296892a0040d19bb2f2ce9eba1e616268d1a5172a7d34581945e33953afa41cf77b08266b34c7a7c6eaa29b4217c6c0c04b27aea361f2779524364d19682799b65cd066273c90834fa4d9bf9cbd87a6503780b50c60bc7b6caa474c6b89ffeafe3b7808b2efe43cc5b69336e06dde33ced40c7eaffb";
/// The throwaway key's second prime.
const Q: &str = "f4618c676be763348898e5ea45e87e54e321af40cc410456a4244cbc66dd2bd7b7b2aeb2fcd3c5a42a5d55bb3b62b3835fc5f53c30de98748e7311b2f2a1cd493b837973ba5f5572941c04192ce53c28cebbdefe204883d573c12f49b20e9cadddabf57b2514c0d31dac68ab54622d822a4030e2bb38100d220104b93e12692b";
/// The throwaway key's private exponent, as MEGA stores it: whole, not pre-reduced.
const D: &str = "1bf1144597230332c2fe1ff6b28373ee2d9f5dcf2ea5a2f2fb740d8a3d934b46a0d6d3617e62cc57962d6f2d2d0b575fcde2965634515003148727188a80c85fc8af475961c69cc5db0ec75972bc80454706c4e3e13b30fd122587313aadc87d43cdeea086b80e708b7350c6e544ba7a3cd81ba46451df909e4a1e6466ed6069ec76cee3af32e625263f71b34a08ab957a94c468111829984a54740e3f1f726610780065c009ad273e5940d9efcd80f861a0fa2414704b53f962c534d94f4832a9884bb9c7819e34c9d45af6a3743d28ec8eaacf00b504f44a6cbca07a5ad8844ec4184cdff91d936ee508aa65b4c8650f0da616314adf5c66f8e861089c105b";
/// `p^-1 mod q`, the fourth MPI MEGA's private key block carries.
const U: &str = "cf54ecebdce74cd701660f22d812c2b46b44f759061c3c67047a4f3e9a94cbda3bf39085c3fb11530468c975bde404151ee5311e3df1dce5d697334728df258072ead8ed5e84b9614437b7741c014f7844ee708261d66e4474ddacd1227ba652355c30e196636cd48cef1c0f2f920db3938c9b3dbd6de36ba25eb9c12b4ee40f";
/// The session identifier as `us` hands it over: 256 bytes of RSA ciphertext.
const CSID: &str = "e39e7abd27984871d62507c5f18dd7285045357068c521d9452ad9d57cce0c789faff860cd1945f3aa35ff7977075223d6f017556e3a5c79d0105362bf31e8bd08985b150ae494de6781d2c9c6d0710b72afabcd642c2bde816f9c4a80ed097daefeac30d57c4357c06639637fd47347cc9bb7aa9e750bca0875a2ec220e01b8f15faa2c13874677d4ca49a17344ff81c6c5849301279707c882ac1446138f508c5b33cf297052d2e089a99667c117bea15e788fbfd587d665961826187e435ea9609f945f10dfb98ede3a1efea586190ec44646e7cb4d97cdfbe3d615a8c080faa69dca7a3c76b591f76cfe30593a7294733701f9db1089eeab3885dad37437";

/// An entry point that does nothing, so the runner can subtract the cost of a call itself.
#[unsafe(no_mangle)]
pub extern "C" fn probe_noop() -> u32 {
    0
}

/// Account version 2: PBKDF2-HMAC-SHA512, 100 000 rounds, 32 bytes out.
///
/// The first sixteen unwrap the master key, the last sixteen are the `uh` the request carries.
#[unsafe(no_mangle)]
pub extern "C" fn probe_pbkdf2_v2() -> u32 {
    checksum(&pbkdf2_hmac_sha512(PASSWORD, SALT, PBKDF2_ROUNDS))
}

/// Account version 1: 65 536 AES rounds to prepare the key, then 16 384 more for the hash.
#[unsafe(no_mangle)]
pub extern "C" fn probe_stringhash_v1() -> u32 {
    let key = prepare_key_v1(PASSWORD);
    let hash = stringhash_v1(b"probe@example.invalid", &key);
    checksum(&[key, hash].concat())
}

/// Unwrapping what `us` answered with: the master key, then the private-key block.
///
/// AES-128-ECB, forty-two blocks in total. Present so the measurement can say what the cheap
/// part costs as well as the expensive ones.
#[unsafe(no_mangle)]
pub extern "C" fn probe_unwrap_keys() -> u32 {
    let derived = [0x11_u8; 16];
    let master = decrypt_ecb(&derived, &WRAPPED_MASTER_KEY);
    let wrapped = vec![0x22_u8; WRAPPED_PRIVATE_KEY_BYTES];
    let private = decrypt_ecb(
        <&[u8; 16]>::try_from(&master[..]).unwrap_or(&[0; 16]),
        &wrapped,
    );
    checksum(&private)
}

/// The one nobody had priced: an RSA-2048 private operation on the session identifier.
///
/// Done the way every real implementation does it, with the Chinese remainder theorem, so the
/// number is the cost of the *cheap* route rather than of a single 2048-bit exponentiation.
#[unsafe(no_mangle)]
pub extern "C" fn probe_rsa_csid() -> u32 {
    checksum(&rsa_decrypt_crt())
}

/// PBKDF2-HMAC-SHA512 for a derived key of at most one hash block.
fn pbkdf2_hmac_sha512(password: &[u8], salt: &[u8], rounds: u32) -> [u8; 32] {
    let round = |data: &[u8]| -> Vec<u8> {
        let mut mac = <Hmac<Sha512> as Mac>::new_from_slice(password)
            .expect("HMAC takes a key of any length");
        mac.update(data);
        mac.finalize().into_bytes().to_vec()
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
    let mut derived = [0_u8; 32];
    derived.copy_from_slice(&accumulated[..32]);
    derived
}

/// MEGA's legacy key preparation: the password encrypts a fixed block 65 536 times over.
fn prepare_key_v1(password: &[u8]) -> [u8; 16] {
    let mut state: [u8; 16] = [
        0x93, 0xC4, 0x67, 0xE3, 0x7D, 0xB0, 0xC7, 0xA4, 0xD1, 0xBE, 0x3F, 0x81, 0x01, 0x52, 0xCB,
        0x56,
    ];
    let mut padded = password.to_vec();
    while !padded.len().is_multiple_of(16) {
        padded.push(0);
    }
    let (blocks, _) = padded.as_chunks::<16>();
    for _ in 0..V1_PREPARE_ROUNDS {
        for key in blocks {
            Aes128::new(key.into()).encrypt_block((&mut state).into());
        }
    }
    state
}

/// MEGA's legacy e-mail hash: 16 384 rounds over the folded address.
fn stringhash_v1(email: &[u8], key: &[u8; 16]) -> [u8; 16] {
    let mut state = [0_u8; 16];
    for (index, byte) in email.iter().enumerate() {
        state[index % 16] ^= *byte;
    }
    let cipher = Aes128::new(key.into());
    for _ in 0..V1_STRINGHASH_ROUNDS {
        cipher.encrypt_block((&mut state).into());
    }
    state
}

/// AES-128-ECB over whole blocks; a trailing partial block is left alone, as MEGA's are not.
fn decrypt_ecb(key: &[u8; 16], data: &[u8]) -> Vec<u8> {
    let cipher = Aes128::new(key.into());
    let mut out = data.to_vec();
    let (blocks, _) = out.as_chunks_mut::<16>();
    for block in blocks {
        cipher.decrypt_block(block.into());
    }
    out
}

/// `m = c^d mod n`, by the two half-width exponentiations every implementation uses.
fn rsa_decrypt_crt() -> Vec<u8> {
    let parse = |text: &str| {
        BigUint::parse_bytes(text.as_bytes(), 16).unwrap_or_else(|| BigUint::from(1_u32))
    };
    let (p, q, d, u, c) = (parse(P), parse(Q), parse(D), parse(U), parse(CSID));
    let one = BigUint::from(1_u32);
    let exponent_p = &d % (&p - &one);
    let exponent_q = &d % (&q - &one);
    let message_p = c.modpow(&exponent_p, &p);
    let message_q = c.modpow(&exponent_q, &q);
    // `(m_q - m_p) * u mod q`, with the subtraction kept non-negative the long way round.
    let difference = (&message_q + &q - (&message_p % &q)) % &q;
    let helper = (difference * u) % &q;
    (message_p + helper * &p).to_bytes_be()
}

/// Folds an output into one word, so the optimiser cannot drop the work that produced it.
fn checksum(bytes: &[u8]) -> u32 {
    bytes.iter().fold(0x811c_9dc5_u32, |state, byte| {
        (state ^ u32::from(*byte)).wrapping_mul(0x0100_0193)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 6070 has no SHA-512 case, so the anchor is the shape rather than a published
    /// vector: one round of PBKDF2 is exactly HMAC over salt with the block index appended.
    #[test]
    fn one_round_of_pbkdf2_is_the_bare_hmac() {
        let derived = pbkdf2_hmac_sha512(b"password", b"salt", 1);
        let mut mac =
            <Hmac<Sha512> as Mac>::new_from_slice(b"password").expect("a key of any length");
        mac.update(b"salt");
        mac.update(&1_u32.to_be_bytes());
        let expected = mac.finalize().into_bytes();
        assert_eq!(derived[..], expected[..32]);
    }

    /// More rounds have to change the answer, or the loop is not running.
    #[test]
    fn more_rounds_derive_something_else() {
        assert_ne!(
            pbkdf2_hmac_sha512(b"password", b"salt", 1),
            pbkdf2_hmac_sha512(b"password", b"salt", 2)
        );
    }

    /// The whole point of the RSA export: it has to recover what was encrypted, or the
    /// measurement is of an exponentiation that went nowhere.
    #[test]
    fn the_crt_route_recovers_the_plaintext() {
        let recovered = rsa_decrypt_crt();
        // The probe's ciphertext was made from the bytes 1..=43 followed by zero padding.
        let expected: Vec<u8> = (1_u8..=43).collect();
        assert_eq!(&recovered[..43], &expected[..]);
        assert!(recovered[43..].iter().all(|byte| *byte == 0));
    }

    /// Neither legacy stage may collapse to a constant.
    #[test]
    fn the_legacy_stages_depend_on_their_input() {
        assert_ne!(prepare_key_v1(b"one"), prepare_key_v1(b"two"));
        let key = prepare_key_v1(b"one");
        assert_ne!(
            stringhash_v1(b"a@b.invalid", &key),
            stringhash_v1(b"c@d.invalid", &key)
        );
    }
}
