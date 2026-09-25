use anyhow::{Context, Result, bail};
use quick_xml::{Reader, XmlVersion, events::Event};

/// Hard upper bound for an imported NZB document.
pub const MAX_NZB_BYTES: usize = 64 * 1024 * 1024;

/// File name announced in an NZB subject, e.g. `[group] - "release.par2" yEnc (1/5)`.
///
/// Posters frequently obfuscate the yEnc `name=` per article while the subject keeps the
/// real name; SABnzbd prefers the subject in that case and so do we.
///
/// Every quoted group is a candidate, and the last one that passes as a file name wins.
/// SABnzbd's `subject_name_extractor` takes the first group unchecked; some posters quote the
/// release name first and the file name second (`"Release.x265-GRP" - [44/50] - "abc.par2"`),
/// and the first group then names fifty rows after the release (RD-108-23). When several
/// groups pass, the release name tends to come first and the file last.
#[must_use]
pub fn subject_file_name(subject: &str) -> Option<String> {
    // The odd-numbered pieces between the quotes; an unterminated trailing quote leaves a
    // final piece that is no group.
    let mut quoted: Vec<&str> = subject.split('"').skip(1).step_by(2).collect();
    if subject.matches('"').count() % 2 == 1 {
        quoted.pop();
    }
    let groups: Vec<&str> = quoted
        .into_iter()
        .map(str::trim)
        .filter(|group| !group.is_empty())
        .collect();
    if groups.is_empty() {
        let bare = subject.trim();
        if bare.contains(char::is_whitespace) {
            return None;
        }
        return acceptable_file_name(bare).then(|| bare.to_owned());
    }
    groups
        .into_iter()
        .rev()
        .find(|candidate| acceptable_file_name(candidate))
        .map(str::to_owned)
}

/// A file name the subject may announce: a short extension, no path separators, not the
/// `yEnc` marker itself.
fn acceptable_file_name(candidate: &str) -> bool {
    looks_like_file_name(candidate)
        && !candidate.contains(['/', '\\'])
        && !candidate.starts_with("yEnc")
}

/// Whether a name carries a short alphanumeric extension (`release.r01`, not `x7f3k`).
#[must_use]
pub fn looks_like_file_name(name: &str) -> bool {
    name.rsplit_once('.').is_some_and(|(stem, extension)| {
        !stem.is_empty()
            && (1..=8).contains(&extension.len())
            && extension.chars().all(char::is_alphanumeric)
    })
}

/// Parsed NZB metadata required by the segment scheduler.
#[derive(Clone, Debug, Default)]
pub struct NzbDocument {
    /// Archive password announced in `<head><meta type="password">`.
    ///
    /// Kept apart from the file metadata: callers carry it onto the package, where extraction
    /// tries it before the shared password list.
    pub password: Option<String>,
    pub files: Vec<NzbFile>,
}

/// One logical file in an NZB.
#[derive(Clone, Debug, Default)]
pub struct NzbFile {
    pub subject: String,
    pub poster: String,
    pub groups: Vec<String>,
    pub segments: Vec<NzbSegment>,
}

/// One NNTP article reference.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NzbSegment {
    pub number: u32,
    pub bytes: u64,
    pub message_id: String,
}

/// Parses an NZB while allowing the standard external NZB doctype, but rejecting
/// internal subsets and entity declarations.
pub fn parse_nzb(input: &[u8]) -> Result<NzbDocument> {
    if input.len() > MAX_NZB_BYTES {
        bail!("NZB exceeds the 64 MiB input limit");
    }

    let mut reader = Reader::from_reader(input);
    reader.config_mut().trim_text(true);
    let mut document = NzbDocument::default();
    let mut current_file: Option<NzbFile> = None;
    let mut current_segment: Option<(u32, u64)> = None;
    let mut current_element = Vec::new();
    let mut in_head = false;
    let mut password_meta: Option<String> = None;
    loop {
        match reader.read_event()? {
            Event::Start(start) => {
                current_element = start.name().as_ref().to_vec();
                match start.name().as_ref() {
                    b"head" => in_head = true,
                    b"meta" if in_head => {
                        let is_password = start.attributes().with_checks(true).try_fold(
                            false,
                            |is_password, attribute| {
                                let attribute = attribute?;
                                Ok::<_, quick_xml::Error>(
                                    is_password
                                        || (attribute.key.as_ref() == b"type"
                                            && attribute
                                                .normalized_value(XmlVersion::Implicit1_0)?
                                                .eq_ignore_ascii_case("password")),
                                )
                            },
                        )?;
                        password_meta = is_password.then(String::new);
                    }
                    b"file" => {
                        let mut file = NzbFile::default();
                        for attribute in start.attributes().with_checks(true) {
                            let attribute = attribute?;
                            match attribute.key.as_ref() {
                                b"subject" => {
                                    file.subject = attribute
                                        .normalized_value(XmlVersion::Implicit1_0)?
                                        .into_owned()
                                }
                                b"poster" => {
                                    file.poster = attribute
                                        .normalized_value(XmlVersion::Implicit1_0)?
                                        .into_owned()
                                }
                                _ => {}
                            }
                        }
                        current_file = Some(file);
                    }
                    b"segment" => {
                        let mut number = None;
                        let mut bytes = None;
                        for attribute in start.attributes().with_checks(true) {
                            let attribute = attribute?;
                            match attribute.key.as_ref() {
                                b"number" => {
                                    number = Some(
                                        attribute
                                            .normalized_value(XmlVersion::Implicit1_0)?
                                            .parse()?,
                                    )
                                }
                                b"bytes" => {
                                    bytes = Some(
                                        attribute
                                            .normalized_value(XmlVersion::Implicit1_0)?
                                            .parse()?,
                                    )
                                }
                                _ => {}
                            }
                        }
                        current_segment = Some((
                            number.context("NZB segment has no number")?,
                            bytes.context("NZB segment has no size")?,
                        ));
                    }
                    _ => {}
                }
            }
            Event::Text(text) => {
                let decoded = text.decode()?;
                let value = quick_xml::escape::unescape(&decoded)?.into_owned();
                if let Some(password) = &mut password_meta {
                    password.push_str(&value);
                }
                match current_element.as_slice() {
                    b"group" => {
                        if let Some(file) = &mut current_file {
                            file.groups.push(value);
                        }
                    }
                    b"segment" => {
                        if let (Some(file), Some((number, bytes))) =
                            (&mut current_file, current_segment.take())
                        {
                            file.segments.push(NzbSegment {
                                number,
                                bytes,
                                message_id: value.trim_matches(['<', '>']).to_owned(),
                            });
                        }
                    }
                    _ => {}
                }
            }
            Event::CData(text) => {
                if let Some(password) = &mut password_meta {
                    password.push_str(&text.decode()?);
                }
            }
            Event::GeneralRef(reference) => {
                if let Some(password) = &mut password_meta {
                    if let Some(character) = reference.resolve_char_ref()? {
                        password.push(character);
                    } else {
                        let entity = reference.decode()?;
                        let value = quick_xml::escape::resolve_xml_entity(&entity)
                            .context("NZB password contains an unknown XML entity")?;
                        password.push_str(value);
                    }
                }
            }
            Event::End(end) => {
                match end.name().as_ref() {
                    b"meta" => {
                        if let Some(password) = password_meta.take() {
                            remember_password(&mut document, &password)?;
                        }
                    }
                    b"head" => in_head = false,
                    b"file" => {
                        if let Some(mut file) = current_file.take() {
                            file.segments.sort_by_key(|segment| segment.number);
                            // Real-world NZBs occasionally repeat a segment or count from 0;
                            // the database enforces `number > 0` and `UNIQUE(file_id, number)`,
                            // so both would otherwise abort the whole import.
                            file.segments.dedup_by_key(|segment| segment.number);
                            if file
                                .segments
                                .first()
                                .is_some_and(|segment| segment.number == 0)
                            {
                                for segment in &mut file.segments {
                                    segment.number += 1;
                                }
                            }
                            document.files.push(file);
                        }
                    }
                    _ => {}
                }
                current_element.clear();
            }
            Event::Eof => break,
            Event::DocType(doctype) => validate_doctype(&doctype.decode()?)?,
            _ => {}
        }
    }
    if document.files.is_empty() {
        bail!("NZB contains no files");
    }
    Ok(document)
}

/// Keeps the first usable password, matching the first-announced-wins rule of other intake
/// formats. The same bounds as the package editor keep untrusted NZB metadata out of command-line
/// arguments with line breaks and prevent an indexer response from creating an unbounded field.
fn remember_password(document: &mut NzbDocument, value: &str) -> Result<()> {
    if document.password.is_some() {
        return Ok(());
    }
    let password = value.trim();
    if password.is_empty() {
        return Ok(());
    }
    if password.chars().count() > 1024 || password.contains(['\n', '\r']) {
        bail!("NZB archive password must be between 1 and 1024 characters on one line");
    }
    document.password = Some(password.to_owned());
    Ok(())
}

fn validate_doctype(value: &str) -> Result<()> {
    let trimmed = value.trim();
    let root = trimmed.split_ascii_whitespace().next().unwrap_or_default();
    if !root.eq_ignore_ascii_case("nzb")
        || trimmed.contains(['[', ']'])
        || trimmed.to_ascii_lowercase().contains("<!entity")
    {
        bail!("only the external NZB doctype is allowed");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::parse_nzb;

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
}
