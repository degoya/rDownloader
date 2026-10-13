use super::*;

fn cipher() -> CipherSpec {
    CipherSpec {
        algorithm: CIPHER_AES_128_CTR.to_owned(),
        key_reference: Some("vault://019d0000-0000-7000-8000-000000000001".to_owned()),
        nonce: vec![0x80, 0x1b, 0x72, 0xfd, 0x96, 0x41, 0xcc, 0xfa],
        first_block: 0,
    }
}

fn integrity() -> IntegritySpec {
    IntegritySpec {
        algorithm: INTEGRITY_CBC_MAC_CHAIN.to_owned(),
        boundaries: vec![131_072, 393_216, 524_288],
        iv: vec![0x11; 16],
        expected: vec![0x22; 8],
    }
}

fn transform() -> ContentTransform {
    ContentTransform {
        cipher: cipher(),
        integrity: Some(integrity()),
    }
}

#[test]
fn a_complete_description_validates() {
    transform().validate().expect("the description is known");
}

#[test]
fn an_unknown_cipher_is_refused_under_its_own_code() {
    let mut description = transform();
    description.cipher.algorithm = "rot13-ctr".to_owned();
    let failure = description.validate().expect_err("refused");
    assert_eq!(failure.code.as_deref(), Some(CODE_CIPHER_UNKNOWN));
    assert!(matches!(failure.category, FailureKind::Permanent));
    assert!(failure.message.contains("rot13-ctr"), "{}", failure.message);
}

#[test]
fn an_unknown_integrity_algorithm_is_refused_under_its_own_code() {
    let mut description = transform();
    if let Some(integrity) = description.integrity.as_mut() {
        integrity.algorithm = "sha3-chain".to_owned();
    }
    let failure = description.validate().expect_err("refused");
    assert_eq!(failure.code.as_deref(), Some(CODE_INTEGRITY_UNKNOWN));
}

#[test]
fn a_refusal_cannot_smuggle_control_characters_into_a_log_line() {
    let mut description = transform();
    description.cipher.algorithm = "aes\n\u{7}INJECTED".to_owned();
    let failure = description.validate().expect_err("refused");
    assert!(!failure.message.contains('\n'), "{}", failure.message);
    assert!(!failure.message.contains('\u{7}'), "{}", failure.message);
}

#[test]
fn bad_parameter_lengths_are_refused() {
    for mutate in [
        (|d: &mut ContentTransform| d.cipher.nonce = vec![0; 7]) as fn(&mut ContentTransform),
        |d: &mut ContentTransform| {
            d.integrity.as_mut().expect("integrity").iv = vec![0; 15];
        },
        |d: &mut ContentTransform| {
            d.integrity.as_mut().expect("integrity").expected = vec![0; 16];
        },
        |d: &mut ContentTransform| {
            d.integrity.as_mut().expect("integrity").boundaries = Vec::new();
        },
        |d: &mut ContentTransform| {
            d.integrity.as_mut().expect("integrity").boundaries = vec![10, 10];
        },
        |d: &mut ContentTransform| {
            d.integrity.as_mut().expect("integrity").boundaries = vec![0, 10];
        },
    ] {
        let mut description = transform();
        mutate(&mut description);
        let failure = description.validate().expect_err("refused");
        assert_eq!(failure.code.as_deref(), Some(CODE_PARAMETERS_INVALID));
    }
}

#[test]
fn a_description_without_integrity_still_validates() {
    let description = ContentTransform {
        cipher: cipher(),
        integrity: None,
    };
    description.validate().expect("a cipher alone is complete");
}

#[test]
fn chunk_ranges_cover_the_file_without_a_gap() {
    let integrity = integrity();
    assert_eq!(integrity.chunk_range(0), Some((0, 131_072)));
    assert_eq!(integrity.chunk_range(1), Some((131_072, 393_216)));
    assert_eq!(integrity.chunk_range(2), Some((393_216, 524_288)));
    assert_eq!(integrity.chunk_range(3), None);
    assert_eq!(integrity.chunk_of(0), Some(0));
    assert_eq!(integrity.chunk_of(131_071), Some(0));
    assert_eq!(integrity.chunk_of(131_072), Some(1));
    assert_eq!(integrity.chunk_of(524_288), None);
}

#[test]
fn a_resume_falls_back_to_the_last_finished_chunk() {
    let integrity = integrity();
    assert_eq!(integrity.aligned_floor(0), 0);
    assert_eq!(integrity.aligned_floor(131_071), 0);
    assert_eq!(integrity.aligned_floor(131_072), 131_072);
    assert_eq!(integrity.aligned_floor(200_000), 131_072);
    assert_eq!(integrity.aligned_floor(524_288), 524_288);
    assert!(integrity.is_boundary(0));
    assert!(integrity.is_boundary(393_216));
    assert!(!integrity.is_boundary(200_000));
}

/// Every parameter that changes the plaintext changes the fingerprint. A continuation is
/// only allowed to reuse somebody's chunk MACs when nothing that produced them moved.
#[test]
fn the_fingerprint_moves_with_every_parameter_that_changes_the_bytes() {
    let base = transform().fingerprint();
    assert_eq!(base.len(), 32);
    assert_eq!(base, transform().fingerprint(), "not deterministic");
    let mutations: [fn(&mut ContentTransform); 6] = [
        |d| d.cipher.nonce[0] ^= 1,
        |d| d.cipher.first_block += 1,
        |d| d.cipher.key_reference = Some("vault://other".to_owned()),
        |d| d.integrity.as_mut().expect("integrity").iv[3] ^= 1,
        |d| d.integrity.as_mut().expect("integrity").expected[0] ^= 1,
        |d| d.integrity.as_mut().expect("integrity").boundaries[0] += 16,
    ];
    for mutate in mutations {
        let mut description = transform();
        mutate(&mut description);
        assert_ne!(base, description.fingerprint());
    }
}

/// The length prefix is what stops two different descriptions hashing the same.
#[test]
fn concatenation_cannot_collide() {
    let mut left = transform();
    left.cipher.algorithm = CIPHER_AES_128_CTR.to_owned();
    left.cipher.key_reference = Some("ab".to_owned());
    let mut right = left.clone();
    right.cipher.key_reference = Some("a".to_owned());
    assert_ne!(left.fingerprint(), right.fingerprint());
}

/// Nothing that can be serialised, printed or persisted carries key bytes.
#[test]
fn a_description_carries_a_reference_and_never_a_key() {
    let json = serde_json::to_string(&transform()).expect("json");
    assert!(json.contains("vault://"), "{json}");
    assert!(!json.contains("key\":["), "{json}");
    let key = TransformKey::new(vec![0xAB; 16]);
    let printed = format!("{key:?}");
    assert!(printed.contains("[redacted]"), "{printed}");
    assert!(!printed.contains("ab"), "{printed}");
    assert!(!printed.contains("171"), "{printed}");
    assert_eq!(key.len(), 16);
    assert!(!key.is_empty());
    assert_eq!(key.expose(), [0xAB; 16]);
}
