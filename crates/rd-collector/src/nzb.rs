use anyhow::{Context, Result, bail};
use quick_xml::{
    Reader, XmlVersion,
    events::{BytesCData, BytesEnd, BytesRef, BytesStart, BytesText, Event},
};

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
    let mut parser = NzbParser::default();
    loop {
        match reader.read_event()? {
            Event::Start(start) => parser.start(start)?,
            Event::Text(text) => parser.text(text)?,
            Event::CData(text) => parser.cdata(text),
            Event::GeneralRef(reference) => parser.general_ref(reference)?,
            Event::End(end) => parser.end(end)?,
            Event::Eof => break,
            Event::DocType(doctype) => validate_doctype(&doctype)?,
            _ => {}
        }
    }
    let document = parser.document;
    if document.files.is_empty() {
        bail!("NZB contains no files");
    }
    Ok(document)
}

/// What `parse_nzb` holds between two events.
#[derive(Default)]
struct NzbParser {
    document: NzbDocument,
    current_file: Option<NzbFile>,
    current_segment: Option<(u32, u64)>,
    current_element: String,
    in_head: bool,
    /// The text of a `<meta type="password">` while it is open.
    password_meta: Option<String>,
}

impl NzbParser {
    fn start(&mut self, start: BytesStart<'_>) -> Result<()> {
        self.current_element = start.name().as_ref().to_owned();
        match start.name().as_ref() {
            "head" => self.in_head = true,
            "meta" if self.in_head => {
                let is_password = is_password_meta(&start)?;
                self.password_meta = is_password.then(String::new);
            }
            "file" => {
                let file = file_start(&start)?;
                self.current_file = Some(file);
            }
            "segment" => {
                let segment = segment_start(&start)?;
                self.current_segment = Some(segment);
            }
            _ => {}
        }
        Ok(())
    }

    fn text(&mut self, text: BytesText<'_>) -> Result<()> {
        let value = quick_xml::escape::unescape(&text)?.into_owned();
        if let Some(password) = &mut self.password_meta {
            password.push_str(&value);
        }
        match self.current_element.as_str() {
            "group" => {
                if let Some(file) = &mut self.current_file {
                    file.groups.push(value);
                }
            }
            "segment" => {
                if let (Some(file), Some((number, bytes))) =
                    (&mut self.current_file, self.current_segment.take())
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
        Ok(())
    }

    fn cdata(&mut self, text: BytesCData<'_>) {
        if let Some(password) = &mut self.password_meta {
            password.push_str(&text);
        }
    }

    fn general_ref(&mut self, reference: BytesRef<'_>) -> Result<()> {
        if let Some(password) = &mut self.password_meta {
            if let Some(character) = reference.resolve_char_ref()? {
                password.push(character);
            } else {
                let value = quick_xml::escape::resolve_xml_entity(&reference)
                    .context("NZB password contains an unknown XML entity")?;
                password.push_str(value);
            }
        }
        Ok(())
    }

    fn end(&mut self, end: BytesEnd<'_>) -> Result<()> {
        match end.name().as_ref() {
            "meta" => {
                if let Some(password) = self.password_meta.take() {
                    remember_password(&mut self.document, &password)?;
                }
            }
            "head" => self.in_head = false,
            "file" => {
                if let Some(file) = self.current_file.take() {
                    self.document.files.push(finished_file(file));
                }
            }
            _ => {}
        }
        self.current_element.clear();
        Ok(())
    }
}

/// Whether a `<meta>` in the head is `type="password"`.
fn is_password_meta(start: &BytesStart<'_>) -> Result<bool> {
    let is_password =
        start
            .attributes()
            .with_checks(true)
            .try_fold(false, |is_password, attribute| {
                let attribute = attribute?;
                Ok::<_, quick_xml::Error>(
                    is_password
                        || (attribute.key.as_ref() == "type"
                            && attribute
                                .normalized_value(XmlVersion::Implicit1_0)?
                                .eq_ignore_ascii_case("password")),
                )
            })?;
    Ok(is_password)
}

/// A `<file>` with its subject and poster.
fn file_start(start: &BytesStart<'_>) -> Result<NzbFile> {
    let mut file = NzbFile::default();
    for attribute in start.attributes().with_checks(true) {
        let attribute = attribute?;
        match attribute.key.as_ref() {
            "subject" => {
                file.subject = attribute
                    .normalized_value(XmlVersion::Implicit1_0)?
                    .into_owned()
            }
            "poster" => {
                file.poster = attribute
                    .normalized_value(XmlVersion::Implicit1_0)?
                    .into_owned()
            }
            _ => {}
        }
    }
    Ok(file)
}

/// A `<segment>`'s number and size; its message id is the text that follows.
fn segment_start(start: &BytesStart<'_>) -> Result<(u32, u64)> {
    let mut number = None;
    let mut bytes = None;
    for attribute in start.attributes().with_checks(true) {
        let attribute = attribute?;
        match attribute.key.as_ref() {
            "number" => {
                number = Some(
                    attribute
                        .normalized_value(XmlVersion::Implicit1_0)?
                        .parse()?,
                )
            }
            "bytes" => {
                bytes = Some(
                    attribute
                        .normalized_value(XmlVersion::Implicit1_0)?
                        .parse()?,
                )
            }
            _ => {}
        }
    }
    Ok((
        number.context("NZB segment has no number")?,
        bytes.context("NZB segment has no size")?,
    ))
}

/// A closed `<file>`, its segments in order, once each, counted from 1.
fn finished_file(mut file: NzbFile) -> NzbFile {
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
    file
}

/// Writes an NZB document back out (RD-191-13).
///
/// An import keeps its files, groups and articles, not the file it came from, so this is the
/// document a provider is handed when an import goes to a remote job instead of the queue. The
/// output is the same for the same input: a second hand-over of one import derives the same
/// content key at the provider and meets the duplicate guard instead of a second job. `name`
/// goes into `<meta type="name">`, where providers read a release name from, and `date` is the
/// per-file attribute the DTD requires, in Unix seconds.
#[must_use]
pub fn render_nzb(document: &NzbDocument, name: Option<&str>, date: i64) -> Vec<u8> {
    use std::fmt::Write as _;

    use quick_xml::escape::escape;

    let mut out = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE nzb PUBLIC \"-//newzBin//DTD NZB 1.1//EN\" \"http://www.newzbin.com/DTD/nzb/nzb-1.1.dtd\">\n\
         <nzb xmlns=\"http://www.newzbin.com/DTD/2003/nzb\">\n",
    );
    let name = name.map(str::trim).filter(|name| !name.is_empty());
    if name.is_some() || document.password.is_some() {
        out.push_str(" <head>\n");
        if let Some(name) = name {
            let _ = writeln!(out, "  <meta type=\"name\">{}</meta>", escape(name));
        }
        if let Some(password) = &document.password {
            let _ = writeln!(out, "  <meta type=\"password\">{}</meta>", escape(password));
        }
        out.push_str(" </head>\n");
    }
    for file in &document.files {
        let _ = writeln!(
            out,
            " <file poster=\"{}\" date=\"{date}\" subject=\"{}\">",
            escape(&file.poster),
            escape(&file.subject)
        );
        out.push_str("  <groups>\n");
        for group in &file.groups {
            let _ = writeln!(out, "   <group>{}</group>", escape(group));
        }
        out.push_str("  </groups>\n  <segments>\n");
        for segment in &file.segments {
            let _ = writeln!(
                out,
                "   <segment bytes=\"{}\" number=\"{}\">{}</segment>",
                segment.bytes,
                segment.number,
                escape(&segment.message_id)
            );
        }
        out.push_str("  </segments>\n </file>\n");
    }
    out.push_str("</nzb>\n");
    out.into_bytes()
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
#[path = "nzb_tests.rs"]
mod tests;
