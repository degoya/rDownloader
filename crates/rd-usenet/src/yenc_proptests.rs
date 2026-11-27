//! The yEnc decoder and the size bound as properties (RD-1490-04).
//!
//! Deterministic and short: a fixed seed, so every run, `--full` included, tries the same cases,
//! and no regression file is written into the source tree. A case a run finds is copied into a
//! fixed test beside the code it concerns (`yenc::tests::a_name_with_spaces_is_kept_whole`).

use crc32fast::hash;
use proptest::{collection::vec, prelude::*, test_runner::RngSeed};

use crate::{DecodedArticle, bounds::declared_size_limit, decode_yenc};

fn config() -> ProptestConfig {
    ProptestConfig {
        cases: 96,
        failure_persistence: None,
        rng_seed: RngSeed::Fixed(0x1490_0004),
        ..ProptestConfig::default()
    }
}

/// yEnc as a poster writes it: lines of `line` characters, the four critical bytes escaped and,
/// as yEnc 1.3 asks, a dot that opens a line and a space or tab that opens or closes one.
fn encode(payload: &[u8], line: usize) -> Vec<u8> {
    let mut encoded = Vec::with_capacity(payload.len() * 2);
    let mut column = 0;
    for (index, byte) in payload.iter().enumerate() {
        let shifted = byte.wrapping_add(42);
        let opens = column == 0;
        let closes = column + 1 >= line || index + 1 == payload.len();
        let critical = matches!(shifted, 0 | b'\n' | b'\r' | b'=')
            || (opens && shifted == b'.')
            || ((opens || closes) && matches!(shifted, b' ' | b'\t'));
        if critical {
            encoded.push(b'=');
            encoded.push(shifted.wrapping_add(64));
            column += 2;
        } else {
            encoded.push(shifted);
            column += 1;
        }
        if column >= line && index + 1 < payload.len() {
            encoded.extend_from_slice(b"\r\n");
            column = 0;
        }
    }
    encoded
}

/// An article that has to decode, its error as the case's failure.
fn must_decode(article: &[u8]) -> Result<DecodedArticle, TestCaseError> {
    decode_yenc(article).map_err(|error| TestCaseError::fail(format!("{error:#}")))
}

/// A file name as posters write them: spaces inside, none at the end.
fn file_name() -> impl Strategy<Value = String> {
    "[A-Za-z0-9._()-][A-Za-z0-9._() -]{0,38}[A-Za-z0-9._()-]"
}

proptest! {
    #![proptest_config(config())]

    /// Whatever a server sends, the decoder answers with a result, never a panic.
    #[test]
    fn arbitrary_bytes_never_panic(bytes in vec(any::<u8>(), 0..2048)) {
        let _ = decode_yenc(&bytes);
    }

    /// A framed article with an arbitrary body: a decoded article holds what its header
    /// announced, and the checksum it reports is the one of the bytes it returns.
    #[test]
    fn an_arbitrary_body_keeps_the_bounds(
        body in vec(any::<u8>(), 0..1024),
        size in 0_u64..1200,
    ) {
        let mut article = format!("=ybegin line=128 size={size} name=x.bin\r\n").into_bytes();
        article.extend_from_slice(&body);
        article.extend_from_slice(format!("\r\n=yend size={size}\r\n").as_bytes());
        if let Ok(decoded) = decode_yenc(&article) {
            prop_assert_eq!(decoded.crc32, hash(&decoded.data));
            if decoded.metadata.part_begin.is_none() {
                prop_assert_eq!(decoded.data.len() as u64, decoded.metadata.declared_size);
            }
            if let Some(expected) = decoded.metadata.expected_crc32 {
                prop_assert_eq!(expected, decoded.crc32);
            }
        }
    }

    /// Encoding and decoding a single-part article returns the bytes, the name and the size.
    #[test]
    fn a_single_part_article_round_trips(
        payload in vec(any::<u8>(), 0..4096),
        line in 1_usize..200,
        name in file_name(),
    ) {
        let mut article = format!(
            "=ybegin line={line} size={} name={name}\r\n",
            payload.len()
        )
        .into_bytes();
        article.extend(encode(&payload, line));
        article.extend_from_slice(
            format!("\r\n=yend size={} crc32={:08x}\r\n", payload.len(), hash(&payload))
                .as_bytes(),
        );
        let decoded = must_decode(&article)?;
        prop_assert_eq!(&decoded.data, &payload);
        prop_assert_eq!(decoded.metadata.name, name);
        prop_assert_eq!(decoded.metadata.declared_size, payload.len() as u64);
        prop_assert_eq!(decoded.crc32, hash(&payload));
    }

    /// One part of a multipart file decodes to its slice and keeps its range; the whole file's
    /// `crc32=` beside the part's `pcrc32=` is not held against the part.
    #[test]
    fn a_part_round_trips(
        payload in vec(any::<u8>(), 2..4096),
        split in any::<prop::sample::Index>(),
        line in 1_usize..200,
    ) {
        let cut = 1 + split.index(payload.len() - 1);
        let part = &payload[cut..];
        let (begin, end) = (cut as u64 + 1, payload.len() as u64);
        let mut article = format!(
            "=ybegin part=2 total=2 line={line} size={} name=multi.bin\r\n=ypart begin={begin} end={end}\r\n",
            payload.len()
        )
        .into_bytes();
        article.extend(encode(part, line));
        article.extend_from_slice(
            format!(
                "\r\n=yend size={} part=2 pcrc32={:08x} crc32={:08x}\r\n",
                part.len(),
                hash(part),
                hash(&payload)
            )
            .as_bytes(),
        );
        let decoded = must_decode(&article)?;
        prop_assert_eq!(decoded.data.as_slice(), part);
        prop_assert_eq!(decoded.metadata.part_begin, Some(begin));
        prop_assert_eq!(decoded.metadata.part_end, Some(end));
        prop_assert_eq!(decoded.metadata.expected_crc32, Some(hash(part)));
    }

    /// The size a header may announce never falls below what the NZB lists and never wraps.
    #[test]
    fn the_declared_size_limit_never_falls_below_the_listed_size(
        listed in any::<u64>(),
        more in any::<u64>(),
    ) {
        let limit = declared_size_limit(listed);
        prop_assert!(limit >= listed);
        prop_assert!(declared_size_limit(listed.saturating_add(more)) >= limit);
    }
}
