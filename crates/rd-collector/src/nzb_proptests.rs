//! The NZB parser as properties (RD-1490-04).
//!
//! Deterministic and short: a fixed seed, so every run, `--full` included, tries the same cases,
//! and no regression file is written into the source tree. A case a run finds is copied into a
//! fixed test beside the parser's own (`nzb::tests::an_entity_keeps_the_whole_value_around_it`).

use proptest::{
    collection::{btree_map, vec},
    option,
    prelude::*,
    sample::select,
    test_runner::RngSeed,
};

use crate::{NzbDocument, NzbFile, NzbSegment, parse_nzb, render_nzb};

/// The date a rendered file without one of its own carries.
const FALLBACK_DATE: i64 = 1_700_000_000;

/// A message id: no white space and no angle brackets, the markup XML escapes included.
const MESSAGE_ID: &str = r#"[A-Za-z0-9.@$%&'"_-]{1,24}"#;
const GROUP: &str = r#"[A-Za-z0-9.&'"<>_-]{1,20}"#;
/// An attribute value: printable ASCII, spaces at either end included.
const ATTRIBUTE: &str = "[ -~]{0,40}";
/// A password as the parser keeps it: trimmed, spaces and markup inside.
const PASSWORD: &str = "[!-~]([ -~]{0,38}[!-~])?";

/// The pieces of a document that is NZB-like enough to reach the parser's own rules.
const PIECES: &[&str] = &[
    "<nzb>",
    "</nzb>",
    "<head>",
    "</head>",
    "<meta type=\"password\">",
    "<meta type=\"name\">",
    "</meta>",
    "<file subject=\"s\" poster=\"p\" date=\"1\">",
    "<file>",
    "</file>",
    "<groups>",
    "</groups>",
    "<group>",
    "</group>",
    "<segments>",
    "</segments>",
    "<segment bytes=\"1\" number=\"0\">",
    "<segment bytes=\"7\" number=\"4294967295\">",
    "<segment number=\"2\" bytes=\"x\">",
    "<segment bytes=\"1\" number=\"-1\">",
    "<segment/>",
    "</segment>",
    "a@b",
    "pass word",
    " ",
    "\n",
    "&amp;",
    "&#10;",
    "&#x41;",
    "&#0;",
    "&unknown;",
    "<![CDATA[x]]>",
    "<!-- c -->",
    "<?xml version=\"1.0\"?>",
    "<!DOCTYPE nzb>",
    "<!DOCTYPE nzb [<!ENTITY a \"b\">]>",
    "<x>",
    "</x>",
    "<",
    ">",
    "\"",
    "&",
];

fn config() -> ProptestConfig {
    ProptestConfig {
        cases: 96,
        failure_persistence: None,
        rng_seed: RngSeed::Fixed(0x1490_0004),
        ..ProptestConfig::default()
    }
}

fn segments() -> impl Strategy<Value = Vec<NzbSegment>> {
    btree_map(1..=u32::MAX, (any::<u64>(), MESSAGE_ID), 0..6).prop_map(|segments| {
        segments
            .into_iter()
            .map(|(number, (bytes, message_id))| NzbSegment {
                number,
                bytes,
                message_id,
            })
            .collect()
    })
}

fn file() -> impl Strategy<Value = NzbFile> {
    (
        ATTRIBUTE,
        ATTRIBUTE,
        option::of(any::<i64>()),
        vec(GROUP, 0..3),
        segments(),
    )
        .prop_map(|(subject, poster, date, groups, segments)| NzbFile {
            subject,
            poster,
            date,
            groups,
            segments,
        })
}

fn document() -> impl Strategy<Value = NzbDocument> {
    (option::of(PASSWORD), vec(file(), 1..4))
        .prop_map(|(password, files)| NzbDocument { password, files })
}

/// What every document the parser accepts holds, whatever it was made from.
fn holds_its_bounds(document: &NzbDocument) -> Result<(), TestCaseError> {
    prop_assert!(!document.files.is_empty());
    for file in &document.files {
        prop_assert!(file.segments.iter().all(|segment| segment.number >= 1));
        prop_assert!(
            file.segments
                .windows(2)
                .all(|pair| pair[0].number < pair[1].number),
            "segments in order, once each"
        );
    }
    if let Some(password) = &document.password {
        prop_assert!(!password.is_empty() && password.chars().count() <= 1024);
        prop_assert!(!password.contains(['\n', '\r']));
        prop_assert_eq!(password.trim(), password.as_str());
    }
    Ok(())
}

proptest! {
    #![proptest_config(config())]

    /// Whatever an indexer or a hot folder hands over, the parser answers with a result.
    #[test]
    fn arbitrary_bytes_never_panic(bytes in vec(any::<u8>(), 0..2048)) {
        if let Ok(document) = parse_nzb(&bytes) {
            holds_its_bounds(&document)?;
        }
    }

    /// NZB-like documents put together from pieces, broken ones among them: never a panic, and
    /// what is accepted keeps its segments in order and its password within its bounds.
    #[test]
    fn nzb_like_pieces_keep_the_bounds(pieces in vec(select(PIECES), 0..48)) {
        if let Ok(document) = parse_nzb(pieces.concat().as_bytes()) {
            holds_its_bounds(&document)?;
        }
    }

    /// A rendered document parses back to what it was made from, markup in every value.
    #[test]
    fn a_rendered_document_round_trips(document in document()) {
        let rendered = render_nzb(&document, Some("Release & Name"), FALLBACK_DATE);
        let parsed = parse_nzb(&rendered)
            .map_err(|error| TestCaseError::fail(format!("{error:#}")))?;
        prop_assert_eq!(&parsed.password, &document.password);
        prop_assert_eq!(parsed.files.len(), document.files.len());
        for (parsed, original) in parsed.files.iter().zip(&document.files) {
            prop_assert_eq!(&parsed.subject, &original.subject);
            prop_assert_eq!(&parsed.poster, &original.poster);
            prop_assert_eq!(parsed.date, Some(original.date.unwrap_or(FALLBACK_DATE)));
            prop_assert_eq!(&parsed.groups, &original.groups);
            prop_assert_eq!(&parsed.segments, &original.segments);
        }
    }
}
