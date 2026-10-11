use super::{NzbDocument, NzbFile, NzbSegment, nzb_refusal_code, parse_nzb, render_nzb};

const BODY: &str = r#"<nzb xmlns="http://www.newzbin.com/DTD/2003/nzb">
      <file poster="tester" subject="example.bin">
        <groups><group>alt.binaries.test</group></groups>
        <segments><segment bytes="42" number="1">message-id@example</segment></segments>
      </file>
    </nzb>"#;

#[test]
fn subject_file_name_prefers_the_quoted_name() {
    assert_eq!(
        super::subject_file_name(r#"[FATX-AMPED] - "ampnjl08.vol07-15.par2" yEnc (1/5)"#)
            .as_deref(),
        Some("ampnjl08.vol07-15.par2")
    );
    assert_eq!(
        super::subject_file_name("example.bin").as_deref(),
        Some("example.bin")
    );
    assert_eq!(
        super::subject_file_name("Some release without a name (1/7)"),
        None
    );
    assert_eq!(super::subject_file_name(r#"bad "../evil.par2" yEnc"#), None);
}

/// The subject from the live finding (RD-108-23): the release name is quoted first, the
/// file name second. `x265-FuN` is no extension, so the first group is no file name -
/// and giving up on it named fifty queue rows after their whole subject line.
#[test]
fn subject_file_name_takes_the_quoted_group_that_is_a_file_name() {
    assert_eq!(
        super::subject_file_name(
            r#""Starfight.1984.German.AC3.DL.1080p.BluRay.x265-FuN" - [44/50] - "amiJ997Yyt9XdW9fApe3pSvlsehAlqpoMKB.vol03+04.par2" yEnc (1/13)"#
        )
        .as_deref(),
        Some("amiJ997Yyt9XdW9fApe3pSvlsehAlqpoMKB.vol03+04.par2")
    );
    // Two groups that both pass as a file name: the release comes first, the file last.
    assert_eq!(
        super::subject_file_name(r#""Show.S01" - [01/10] - "abc.part01.rar" yEnc (1/50)"#)
            .as_deref(),
        Some("abc.part01.rar")
    );
    // No group passes: nothing is guessed, the caller keeps its own fallback.
    assert_eq!(
        super::subject_file_name(r#""Show S01" - [01/10] - "readme" yEnc (1/1)"#),
        None
    );
    // An unterminated quote is not a group.
    assert_eq!(
        super::subject_file_name(r#""Show.S01" - [01/10] - "abc.part01.rar yEnc (1/50)"#)
            .as_deref(),
        Some("Show.S01")
    );
}

#[test]
fn accepts_the_standard_external_nzb_doctype() {
    let input = format!(
        "<?xml version=\"1.0\"?><!DOCTYPE nzb PUBLIC \"-//newzBin//DTD NZB 1.1//EN\" \"http://www.newzbin.com/DTD/nzb/nzb-1.1.dtd\">{BODY}"
    );
    let parsed = parse_nzb(input.as_bytes()).expect("standard NZB doctype");
    assert_eq!(parsed.files.len(), 1);
    assert_eq!(parsed.files[0].segments.len(), 1);
}

#[test]
fn reads_the_archive_password_from_the_nzb_head() {
    let input = BODY.replacen(
        "<nzb xmlns=\"http://www.newzbin.com/DTD/2003/nzb\">",
        "<nzb xmlns=\"http://www.newzbin.com/DTD/2003/nzb\"><head>\
             <meta type=\"category\">movies</meta>\
             <meta type=\"PASSWORD\"> hunter&amp;2 </meta></head>",
        1,
    );
    let parsed = parse_nzb(input.as_bytes()).expect("NZB with password metadata");

    assert_eq!(parsed.password.as_deref(), Some("hunter&2"));
}

#[test]
fn parses_the_legal_sabnzbd_fixture() {
    let input = include_bytes!("../../../testfile/sabnzbd-test-download-100MB.nzb");
    let parsed = parse_nzb(input).expect("official SABnzbd test NZB");

    assert_eq!(parsed.files.len(), 13);
    assert_eq!(
        parsed
            .files
            .iter()
            .map(|file| file.segments.len())
            .sum::<usize>(),
        163
    );
    assert!(
        parsed
            .files
            .iter()
            .all(|file| file.groups == ["alt.binaries.test"])
    );
}

#[test]
fn deduplicates_and_renumbers_broken_segments() {
    let input = r#"<nzb xmlns="http://www.newzbin.com/DTD/2003/nzb">
          <file poster="tester" subject="example.bin">
            <groups><group>alt.binaries.test</group></groups>
            <segments>
              <segment bytes="42" number="1">a@example</segment>
              <segment bytes="42" number="1">a-again@example</segment>
              <segment bytes="42" number="0">zero@example</segment>
            </segments>
          </file>
        </nzb>"#;
    let parsed = parse_nzb(input.as_bytes()).expect("broken numbering is tolerated");
    let numbers: Vec<u32> = parsed.files[0]
        .segments
        .iter()
        .map(|segment| segment.number)
        .collect();
    assert_eq!(numbers, vec![1, 2]);
}

#[test]
fn rejects_internal_entity_declarations() {
    let input = format!(
        "<?xml version=\"1.0\"?><!DOCTYPE nzb [<!ENTITY xxe SYSTEM \"file:///etc/passwd\">]>{BODY}"
    );
    let error = parse_nzb(input.as_bytes()).expect_err("internal subset must be rejected");
    assert!(error.to_string().contains("external NZB doctype"));
}

/// What an import hands a provider reads back as the import it came from: every file,
/// group, article and the password, with markup in the values escaped rather than
/// interpreted (RD-191-13).
#[test]
fn a_rendered_document_parses_back_to_what_it_was_made_from() {
    let document = NzbDocument {
        password: Some("p<&>\"ss".to_owned()),
        files: vec![
            NzbFile {
                subject: "[1/2] - \"Show.S01E01.part1.rar\" yEnc (1/2)".to_owned(),
                poster: "Poster <poster@example.test>".to_owned(),
                date: None,
                groups: vec!["alt.binaries.test".to_owned(), "a.b.other".to_owned()],
                segments: vec![
                    NzbSegment {
                        number: 1,
                        bytes: 739_000,
                        message_id: "part1of2@example.test".to_owned(),
                    },
                    NzbSegment {
                        number: 2,
                        bytes: 12,
                        message_id: "part2of2@example.test".to_owned(),
                    },
                ],
            },
            NzbFile {
                subject: "Show.S01E01.par2".to_owned(),
                poster: "poster".to_owned(),
                date: Some(1_600_000_000),
                groups: vec!["alt.binaries.test".to_owned()],
                segments: vec![NzbSegment {
                    number: 1,
                    bytes: 42,
                    message_id: "par@example.test".to_owned(),
                }],
            },
        ],
    };
    let rendered = render_nzb(&document, Some("Show.S01E01"), 1_700_000_000);
    assert_eq!(
        rendered,
        render_nzb(&document, Some("Show.S01E01"), 1_700_000_000),
        "the same import renders the same bytes"
    );
    let text = String::from_utf8(rendered.clone()).expect("UTF-8");
    assert!(
        text.contains("<meta type=\"name\">Show.S01E01</meta>"),
        "{text}"
    );
    // The file without a post date of its own takes the one given, the other keeps its own.
    assert!(text.contains("date=\"1700000000\""), "{text}");
    assert!(text.contains("date=\"1600000000\""), "{text}");
    let parsed = parse_nzb(&rendered).expect("the rendered document parses");
    assert_eq!(parsed.password, document.password);
    assert_eq!(parsed.files.len(), 2);
    for (parsed, original) in parsed.files.iter().zip(&document.files) {
        assert_eq!(parsed.subject, original.subject);
        assert_eq!(parsed.poster, original.poster);
        assert_eq!(parsed.groups, original.groups);
        assert_eq!(parsed.segments, original.segments);
    }
    assert_eq!(parsed.files[1].date, Some(1_600_000_000));
}

/// An exported NZB carries the date its files were posted on, not the time it was written
/// (RD-1240-33); a date that is no number is dropped rather than refusing the document.
#[test]
fn a_rendered_document_keeps_the_post_date_it_was_parsed_with() {
    let posted = BODY.replace("poster=\"tester\"", "poster=\"tester\" date=\"1500000000\"");
    let document = parse_nzb(posted.as_bytes()).expect("fixture");
    assert_eq!(document.files[0].date, Some(1_500_000_000));
    let text = String::from_utf8(render_nzb(&document, None, 1_700_000_000)).expect("UTF-8");
    assert!(text.contains("date=\"1500000000\""), "{text}");
    assert!(!text.contains("1700000000"), "{text}");

    let garbled = BODY.replace("poster=\"tester\"", "poster=\"tester\" date=\"yesterday\"");
    let document = parse_nzb(garbled.as_bytes()).expect("a garbled date is no reason to refuse");
    assert_eq!(document.files[0].date, None);
}

#[test]
fn a_document_without_name_or_password_has_no_head() {
    let document = parse_nzb(BODY.as_bytes()).expect("fixture");
    let text = String::from_utf8(render_nzb(&document, Some("  "), 0)).expect("UTF-8");
    assert!(!text.contains("<head>"), "{text}");
    assert!(
        text.contains("<nzb"),
        "a provider sniffs this element: {text}"
    );
}

/// A failed import's reason carries a code the interface translates (RD-1240-33): the parser's
/// own message, wherever a caller's context put it in the line; anything else has none.
#[test]
fn a_refusal_of_this_parser_is_found_by_its_code() {
    let empty = parse_nzb(br#"<nzb xmlns="http://www.newzbin.com/DTD/2003/nzb"></nzb>"#)
        .expect_err("an NZB without files is refused");
    assert_eq!(
        nzb_refusal_code(&format!("hotfolder intake: {empty:#}")),
        Some("collector.nzb_empty")
    );
    let input = format!("<!DOCTYPE nzb [<!ENTITY x \"y\">]>{BODY}");
    let doctype = parse_nzb(input.as_bytes()).expect_err("an internal subset is refused");
    assert_eq!(
        nzb_refusal_code(&doctype.to_string()),
        Some("nzb.doctype_refused")
    );
    assert_eq!(nzb_refusal_code("permission denied"), None);
}
