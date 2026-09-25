//! The shape, the price and the arithmetic of a derivation (RD-120-20).

use super::{
    AES_ECB_FUEL_PER_BLOCK, CALL_FUEL, MIN_PBKDF2_ROUNDS, PBKDF2_FUEL_PER_ROUND, SecretOrigin,
    fuel_cost, run, validate,
};
use rd_plugin_api::DerivationStep;

/// RD-120-20's tests, all of which are about a typed credential.
const PERSON: SecretOrigin = SecretOrigin::Person;

fn pbkdf2(rounds: u32, length: u32) -> DerivationStep {
    DerivationStep::Pbkdf2HmacSha512 {
        salt: b"mega-login-probe-salt-0123456789".to_vec(),
        rounds,
        length,
    }
}

#[test]
fn a_chain_that_does_not_begin_one_way_is_refused() {
    // Both of these would hand the credential back, one immediately and one under a key
    // an attacker chose. Neither is a derivation.
    for steps in [
        vec![DerivationStep::Take {
            offset: 0,
            length: 8,
        }],
        vec![DerivationStep::Aes128EcbDecrypt(vec![0; 16])],
    ] {
        let error = validate(&steps, PERSON).expect_err("must be refused");
        assert_eq!(
            error.code.as_deref(),
            Some("plugin.key_derivation_needs_one_way")
        );
    }
}

#[test]
fn a_cheap_derivation_is_refused_because_it_would_be_an_oracle() {
    let error = validate(&[pbkdf2(1, 32)], PERSON).expect_err("must be refused");
    assert_eq!(
        error.code.as_deref(),
        Some("plugin.key_derivation_rounds_too_low")
    );
    assert!(validate(&[pbkdf2(MIN_PBKDF2_ROUNDS, 32)], PERSON).is_ok());
}

#[test]
fn an_empty_chain_is_refused() {
    let error = validate(&[], PERSON).expect_err("must be refused");
    assert_eq!(
        error.code.as_deref(),
        Some("plugin.key_derivation_steps_invalid")
    );
}

#[test]
fn the_price_is_the_measured_guest_price() {
    // RD-120-11 measured PBKDF2-HMAC-SHA512 with 100 000 rounds and 32 bytes of output
    // at 3 338 300 549 fuel inside the sandbox. What the host charges must be that
    // number, not a fraction of it: a cheaper price would be a way around the cap.
    let charged = fuel_cost(&[pbkdf2(100_000, 32)]);
    assert_eq!(charged, CALL_FUEL + 100_000 * PBKDF2_FUEL_PER_ROUND);
    let measured = 3_338_300_549_u64;
    assert!(
        charged.abs_diff(measured) * 1_000_000 < measured,
        "charged {charged} is more than a millionth away from the measured {measured}"
    );
    // And the cheap stage in the same proportion: 42 blocks cost 342 965 in the guest.
    let unwrap = fuel_cost(&[
        pbkdf2(100_000, 32),
        DerivationStep::Aes128EcbDecrypt(vec![0; 42 * 16]),
    ]);
    assert_eq!(
        unwrap - charged,
        42 * AES_ECB_FUEL_PER_BLOCK,
        "the AES stage is charged per block"
    );
}

#[test]
fn a_window_keeps_the_half_the_provider_is_meant_to_see() {
    // MEGA's shape: 32 bytes out, the last sixteen are `uh` and the first sixteen never
    // leave. What comes back is the window, and nothing around it.
    let steps = vec![
        pbkdf2(MIN_PBKDF2_ROUNDS, 32),
        DerivationStep::Take {
            offset: 16,
            length: 16,
        },
    ];
    validate(&steps, PERSON).expect("a well-formed chain");
    let whole = run(b"correct horse battery", &[pbkdf2(MIN_PBKDF2_ROUNDS, 32)])
        .expect("the whole derivation");
    let half = run(b"correct horse battery", &steps).expect("the windowed derivation");
    assert_eq!(half.len(), 16);
    assert_eq!(half, whole[16..]);
    assert_ne!(half, whole[..16]);
}

#[test]
fn nothing_of_the_credential_comes_back() {
    // The property the whole primitive rests on, stated as a test: for every chain the
    // host accepts, the credential is not a substring of the answer.
    let secret = b"correct horse battery staple";
    for steps in [
        vec![pbkdf2(MIN_PBKDF2_ROUNDS, 64)],
        vec![
            pbkdf2(MIN_PBKDF2_ROUNDS, 32),
            DerivationStep::Take {
                offset: 0,
                length: 16,
            },
            DerivationStep::Aes128EcbDecrypt(vec![0x22; 16]),
        ],
    ] {
        validate(&steps, PERSON).expect("a well-formed chain");
        let out = run(secret, &steps).expect("the derivation");
        assert!(
            !out.windows(secret.len()).any(|window| window == secret),
            "the credential appeared in the answer"
        );
    }
}

#[test]
fn a_window_past_the_running_value_is_refused_rather_than_padded() {
    let steps = vec![
        pbkdf2(MIN_PBKDF2_ROUNDS, 16),
        DerivationStep::Take {
            offset: 8,
            length: 16,
        },
    ];
    validate(&steps, PERSON).expect("shape is fine; the length only shows when it runs");
    let error = run(b"secret", &steps).expect_err("must be refused");
    assert_eq!(
        error.code.as_deref(),
        Some("plugin.key_derivation_steps_invalid")
    );
}
