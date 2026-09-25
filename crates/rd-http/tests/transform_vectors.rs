//! Fixed vectors for the stream transform (RD-110-33).
//!
//! Every number here was produced outside this crate, by a scratch AES-128 written from the
//! FIPS-197 tables and checked against the standard's own C.1 vector before it was used for
//! anything else. That matters: a test that computes the expectation with the same code it is
//! testing proves only that the code is consistent with itself.
//!
//! Three anchors, in order of how much they pin down:
//!
//! 1. **NIST SP 800-38A F.5.1**, the published CTR-AES128 vector, reproduced by the counter
//!    construction this contract describes -- an 8-byte nonce followed by a big-endian u64
//!    block index. If that shape were wrong, this one case would say so.
//! 2. **A short stream under the measured MEGA parameters**, verbatim in both directions.
//! 3. **A 512 KiB stream**, for the chunked MAC: three chunk MACs, the condensed value, and
//!    the eight bytes it folds to.

use std::collections::BTreeMap;

use rd_core::{
    CIPHER_AES_128_CTR, CODE_INTEGRITY_MISMATCH, CipherSpec, ContentTransform,
    INTEGRITY_CBC_MAC_CHAIN, IntegritySpec, TransformKey,
};
use rd_http::{StreamTransform, TransformCheckpoint};

/// The file key of the public MEGA example the ADR's measurements were taken from.
const KEY: &str = "0c4c44e128eaee7a40bcbd4ffec19617";
/// Its counter prefix.
const NONCE: &str = "801b72fd9641ccfa";

fn hex(value: &str) -> Vec<u8> {
    hex::decode(value).expect("a hex literal in this file")
}

fn transform(
    key: &str,
    nonce: &str,
    first_block: u64,
    integrity: Option<IntegritySpec>,
) -> StreamTransform {
    StreamTransform::new(
        ContentTransform {
            cipher: CipherSpec {
                algorithm: CIPHER_AES_128_CTR.to_owned(),
                key_reference: Some("vault://fixture".to_owned()),
                nonce: hex(nonce),
                first_block,
            },
            integrity,
        },
        &TransformKey::new(hex(key)),
    )
    .expect("the fixture describes primitives this build implements")
}

/// The plaintext of the short vector: non-repeating, with a period coprime to 16, so a
/// keystream applied at the wrong offset produces different bytes rather than lucky ones.
fn short_plaintext() -> Vec<u8> {
    (0..300)
        .map(|index| ((index * 7 + 3) % 251) as u8)
        .collect()
}

/// The plaintext of the 512 KiB vector.
fn long_plaintext() -> Vec<u8> {
    (0..512 * 1024).map(|index| (index % 251) as u8).collect()
}

fn sha256(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(bytes))
}

/// The published CTR-AES128 vector, reproduced by nonce-plus-block-index.
///
/// NIST's initial counter block is `f0f1f2f3f4f5f6f7f8f9fafbfcfdfeff`, which this contract
/// expresses as the nonce `f0f1f2f3f4f5f6f7` and a first block index of `f8f9fafbfcfdfeff`.
#[test]
fn the_counter_construction_reproduces_the_nist_vector() {
    let transform = transform(
        "2b7e151628aed2a6abf7158809cf4f3c",
        "f0f1f2f3f4f5f6f7",
        u64::from_be_bytes(hex("f8f9fafbfcfdfeff").try_into().expect("eight bytes")),
        None,
    );
    let plaintext = hex(concat!(
        "6bc1bee22e409f96e93d7e117393172a",
        "ae2d8a571e03ac9c9eb76fac45af8e51",
        "30c81c46a35ce411e5fbc1191a0a52ef",
        "f69f2445df4f9b17ad2b417be66c3710",
    ));
    let mut buffer = plaintext.clone();
    transform.apply(0, &mut buffer);
    assert_eq!(
        hex::encode(&buffer),
        concat!(
            "874d6191b620e3261bef6864990db6ce",
            "9806f66b7970fdff8617187bb9fffdff",
            "5ae4df3edbd5d35e5b4f09020db03eab",
            "1e031dda2fbe03d1792170a0f3009cee",
        )
    );
    // CTR is its own inverse, which is why one routine serves both directions.
    transform.apply(0, &mut buffer);
    assert_eq!(buffer, plaintext);
}

/// A short stream, verbatim in both directions.
#[test]
fn a_described_stream_is_transformed_byte_for_byte() {
    let transform = transform(KEY, NONCE, 0, None);
    let ciphertext = hex(concat!(
        "e5fea8e44a188a5f96110b0e3dd0d7059a9c64a992e2975f1715fcb396af5d57",
        "add203a79746f7f92665822bf2059a183b56d7768ded17af55027c2d68b758e4",
        "ee3daf0883433e880081e92028479f222d69f8c2e695942b78e6c8a16afb335e",
        "10da935d411276188853eeac32fc77254fa00620408fcc035b53734f881111f1",
        "f26a91aa2968ab354a69142e3e263db5191f914368f0bd0d2efd9ad5a24865e3",
        "8daf63298bcdca1c26fceb4f0aa51a1acbde76f5da9603b64410a0c186b6a422",
        "67b8839431d6a2e371a66b8e4c31215d2efd4f66348495a27eaffcc1bd969ca2",
        "d3c072c636d42b95903995f449ac45c0616f809cf096d8cf1353ebbc0e1baa44",
        "31f29ddd195c0a245059ad6e352380f759b9e436ed42b192e9f4dd4230dffa00",
        "80586472af7bbf1ea9617b78",
    ));
    let mut buffer = ciphertext.clone();
    transform.apply(0, &mut buffer);
    assert_eq!(buffer, short_plaintext());
}

/// The same stream, delivered in the pieces a network delivers.
///
/// The offsets are deliberately not multiples of the block size: a decryption that only works
/// on aligned buffers would pass the case above and corrupt every real download.
#[test]
fn an_unaligned_split_transforms_to_the_same_bytes() {
    let transform = transform(KEY, NONCE, 0, None);
    let plaintext = short_plaintext();
    let mut ciphertext = plaintext.clone();
    transform.apply(0, &mut ciphertext);
    for split in [1_usize, 5, 15, 16, 17, 31, 33, 128, 299] {
        let mut rebuilt = Vec::new();
        let mut position = 0_u64;
        for piece in ciphertext.chunks(split) {
            let mut piece = piece.to_vec();
            transform.apply(position, &mut piece);
            position += piece.len() as u64;
            rebuilt.extend_from_slice(&piece);
        }
        assert_eq!(rebuilt, plaintext, "split of {split} bytes");
    }
}

/// A chunk that starts in the middle of the file decrypts without a preceding byte, which is
/// the property the whole design rests on: ranges, resume and parallel chunks all need it.
#[test]
fn a_chunk_from_the_middle_needs_nothing_before_it() {
    let transform = transform(KEY, NONCE, 0, None);
    let plaintext = long_plaintext();
    let mut whole = plaintext.clone();
    transform.apply(0, &mut whole);
    let mut slice = whole[131_056..131_088].to_vec();
    transform.apply(131_056, &mut slice);
    assert_eq!(
        hex::encode(&slice),
        "22232425262728292a2b2c2d2e2f303132333435363738393a3b3c3d3e3f4041"
    );
}

fn long_integrity(boundaries: Vec<u64>, expected: &str) -> IntegritySpec {
    IntegritySpec {
        algorithm: INTEGRITY_CBC_MAC_CHAIN.to_owned(),
        boundaries,
        iv: [hex(NONCE), hex(NONCE)].concat(),
        expected: hex(expected),
    }
}

/// Walks a whole plaintext through the accumulator and returns every chunk MAC it closed.
fn walk(transform: &StreamTransform, plaintext: &[u8], split: usize) -> BTreeMap<usize, [u8; 16]> {
    let mut walker = transform
        .mac_walker(0)
        .expect("an integrity value was described");
    let mut macs = BTreeMap::new();
    let mut position = 0_u64;
    for piece in plaintext.chunks(split) {
        for (index, mac) in walker.feed(position, piece).expect("in order") {
            macs.insert(index, mac);
        }
        position += piece.len() as u64;
    }
    macs
}

/// The chunk MACs, the condensed value and the eight bytes it folds to.
#[test]
fn the_chunk_mac_chain_matches_the_recomputed_vector() {
    let transform = transform(
        KEY,
        NONCE,
        0,
        Some(long_integrity(
            vec![131_072, 393_216, 524_288],
            "e8f044483f428b92",
        )),
    );
    let plaintext = long_plaintext();
    assert_eq!(
        sha256(&plaintext),
        "61d1d9c5745bdaa4fab39240651bc242a5186b15393fd475082fcf6e84f400ab"
    );
    // Split at a size that is neither the block size nor a chunk boundary, so a chunk MAC is
    // closed in the middle of a delivery the way it is in a real download.
    let macs = walk(&transform, &plaintext, 9_973);
    assert_eq!(
        macs.values().map(hex::encode).collect::<Vec<_>>(),
        vec![
            "e1edb23ddeaba9b39881c00f1365eedb".to_owned(),
            "bec6a0246f2615324e8edb3a3c67dbde".to_owned(),
            "7e41cce5413dcd51888f48a16079b843".to_owned(),
        ]
    );
    transform.verify(&macs).expect("the fixture's own value");
}

/// The same plaintext under a different chunking produces different chunk MACs and a
/// different condensed value, which is why the boundaries are part of the fingerprint.
#[test]
fn a_different_chunking_condenses_to_a_different_value() {
    let transform = transform(
        KEY,
        NONCE,
        0,
        Some(long_integrity(
            vec![131_072, 262_144, 393_216, 524_288],
            "42e920fd56fd5e0b",
        )),
    );
    let macs = walk(&transform, &long_plaintext(), 65_536);
    assert_eq!(
        macs.values().map(hex::encode).collect::<Vec<_>>(),
        vec![
            "e1edb23ddeaba9b39881c00f1365eedb".to_owned(),
            "48b0e39eb272309c3fe930797c856f28".to_owned(),
            "2ef746c8bf16bd22f50a1647ff933b75".to_owned(),
            "7e41cce5413dcd51888f48a16079b843".to_owned(),
        ]
    );
    transform.verify(&macs).expect("the fixture's own value");
}

/// A chunk shorter than the block size is padded with zeros, which is what the provider's
/// own definition says and what the 300-byte vector pins down.
#[test]
fn a_partial_last_block_is_padded_with_zeros() {
    let transform = transform(
        KEY,
        NONCE,
        0,
        Some(long_integrity(vec![300], "5ed499c42f1db962")),
    );
    let macs = walk(&transform, &short_plaintext(), 7);
    assert_eq!(macs.len(), 1);
    transform.verify(&macs).expect("the fixture's own value");
}

/// A wrong integrity value is refused under its own stable code, and the refusal says nothing
/// about how close it was.
#[test]
fn a_wrong_integrity_value_is_refused_under_its_own_code() {
    let transform = transform(
        KEY,
        NONCE,
        0,
        Some(long_integrity(vec![300], "0000000000000000")),
    );
    let macs = walk(&transform, &short_plaintext(), 300);
    let failure = transform.verify(&macs).expect_err("refused");
    assert_eq!(failure.code.as_deref(), Some(CODE_INTEGRITY_MISMATCH));
    assert!(!failure.message.contains("5ed499"), "{}", failure.message);
    assert!(!failure.message.contains("0000"), "{}", failure.message);
}

/// A missing chunk MAC is not quietly treated as a match.
#[test]
fn a_chunk_that_was_never_accounted_for_fails_the_check() {
    let transform = transform(
        KEY,
        NONCE,
        0,
        Some(long_integrity(
            vec![131_072, 393_216, 524_288],
            "e8f044483f428b92",
        )),
    );
    let mut macs = walk(&transform, &long_plaintext(), 65_536);
    macs.remove(&1);
    let failure = transform.verify(&macs).expect_err("refused");
    assert_eq!(
        failure.code.as_deref(),
        Some(rd_core::CODE_CHECKPOINT_MISMATCH)
    );
}

/// Chunk MACs written by another description are not adopted.
#[test]
fn chunk_macs_are_only_adopted_under_the_fingerprint_that_wrote_them() {
    let integrity = long_integrity(vec![131_072, 393_216, 524_288], "e8f044483f428b92");
    let mine = transform(KEY, NONCE, 0, Some(integrity.clone()));
    let checkpoint = TransformCheckpoint {
        fingerprint: Some(mine.fingerprint().to_owned()),
        macs: vec![(0, [7_u8; 16])],
    };
    assert_eq!(mine.adopt(&checkpoint).len(), 1);

    // One bit of a different nonce is a different description, and its chunk MACs describe
    // other plaintext. The key itself is not in the fingerprint because it is not in the
    // description: the vault reference stands for it, and a different key is a different
    // reference.
    let other = transform(KEY, "801b72fd9641ccfb", 0, Some(integrity));
    assert!(other.adopt(&checkpoint).is_empty());
    assert!(
        mine.adopt(&TransformCheckpoint {
            fingerprint: None,
            macs: vec![(0, [7_u8; 16])],
        })
        .is_empty()
    );
}
