//! The replaced rule, both halves, and the two ways round it that the replacement has to shut.

use aes::{
    Aes128,
    cipher::{BlockEncrypt, KeyInit},
};
use rd_plugin_api::DerivationStep;

use super::SecretOrigin;
use crate::keyderive::{MIN_PBKDF2_ROUNDS, run, validate};

/// An invented master key. Sixteen bytes, as a sign-in stores them.
const MASTER_KEY: [u8; 16] = *b"invented-master!";

fn pbkdf2() -> DerivationStep {
    DerivationStep::Pbkdf2HmacSha512 {
        salt: b"origin-tests-salt".to_vec(),
        rounds: MIN_PBKDF2_ROUNDS,
        length: 32,
    }
}

fn aes(data: Vec<u8>) -> DerivationStep {
    DerivationStep::Aes128EcbDecrypt(data)
}

fn take(offset: u32, length: u32) -> DerivationStep {
    DerivationStep::Take { offset, length }
}

/// What a provider hands over: `plain` wrapped under `key`, block by block.
fn wrap(key: &[u8; 16], plain: &[u8]) -> Vec<u8> {
    let cipher = Aes128::new(key.into());
    let mut out = plain.to_vec();
    for block in out.as_chunks_mut::<16>().0 {
        cipher.encrypt_block(block.into());
    }
    out
}

fn code(result: Result<(), rd_core::Failure>) -> Option<String> {
    result.err().and_then(|failure| failure.code)
}

#[test]
fn a_symmetric_first_step_over_a_typed_credential_is_still_refused() {
    // Acceptance criterion 1 of RD-120-30. Exactly the chain the sign-in half now admits,
    // aimed at what a person typed: AES under the password's first sixteen bytes is one block
    // of work per guess, which is the oracle RD-120-20 closed and this job must not reopen.
    assert_eq!(
        code(validate(&[aes(vec![0; 16])], SecretOrigin::Person)).as_deref(),
        Some("plugin.key_derivation_needs_one_way")
    );
    // And the rest of RD-120-20's rule stands as it was.
    assert!(validate(&[pbkdf2()], SecretOrigin::Person).is_ok());
}

#[test]
fn a_symmetric_first_step_over_sign_in_key_material_is_accepted_and_unwraps() {
    // Acceptance criterion 2. MEGA's shape: a node key wrapped under the master key.
    let node_key: Vec<u8> = (0_u8..32).collect();
    let steps = [aes(wrap(&MASTER_KEY, &node_key))];
    validate(&steps, SecretOrigin::SignIn).expect("the sign-in half admits it");
    assert_eq!(run(&MASTER_KEY, &steps).expect("the unwrap"), node_key);
}

#[test]
fn a_window_first_is_refused_whatever_the_origin() {
    for origin in [SecretOrigin::Person, SecretOrigin::SignIn] {
        assert!(
            validate(&[take(0, 16)], origin).is_err(),
            "{origin:?} admitted the credential itself"
        );
    }
}

#[test]
fn a_window_in_front_of_the_key_step_is_refused_because_it_recovers_the_key_bytewise() {
    // The attack the replacement has to shut, spelled out. Were `take` allowed before AES, a
    // guest could key AES with fifteen bytes it already knows and one it does not, compare the
    // answer against its own 256 trial decryptions, and read the stored key one byte at a
    // time. The key step therefore keys itself with the *whole* stored value, never a window.
    let steps = [take(1, 16), aes(vec![0; 16])];
    assert_eq!(
        code(validate(&steps, SecretOrigin::SignIn)).as_deref(),
        Some("plugin.key_derivation_needs_key_step")
    );
}

#[test]
fn a_one_way_step_over_sign_in_key_material_is_refused_as_pointless() {
    // Harmless, and refused all the same: the contract is not a construction kit, and a chain
    // this module has no use for is a shape nobody has argued is safe.
    assert_eq!(
        code(validate(&[pbkdf2()], SecretOrigin::SignIn)).as_deref(),
        Some("plugin.key_derivation_needs_key_step")
    );
}

#[test]
fn what_a_hostile_guest_gains_is_a_decryption_under_the_key_and_never_the_key() {
    // The honest answer to "what can a hostile guest now do that it could not": ask for
    // decryptions under the stored key. That is the widening, and the whole of it. None of
    // the answers is the key -- not for zeros, not for the key fed back in as ciphertext, not
    // with a second AES step keyed by the first one's output.
    let chains = [
        vec![aes(vec![0; 64])],
        vec![aes(MASTER_KEY.to_vec())],
        vec![aes(vec![0xff; 16]), aes(MASTER_KEY.to_vec())],
        vec![aes(vec![0x5a; 32]), take(0, 16)],
    ];
    for steps in chains {
        validate(&steps, SecretOrigin::SignIn).expect("an admitted chain");
        let answer = run(&MASTER_KEY, &steps).expect("computed");
        assert!(
            !answer.windows(16).any(|window| window == MASTER_KEY),
            "the stored key came back from {steps:?}"
        );
    }
}
