//! Which file a Google Workspace document turns into.
//!
//! A Doc, a Sheet or a Slide is not a file: it has no bytes and no size until somebody names
//! the format it should become. So this is the one part of the plugin that makes a *choice*,
//! and it is the part that has to be visible before anything is queued — the name and the
//! extension in the LinkGrabber are the only place a person can notice that their spreadsheet
//! is about to arrive as a PDF.
//!
//! The choice is made in two steps and never guessed at: the address may name a format —
//! Google's own export links spell it `?format=docx` — and otherwise the document type's
//! default applies. A type that exports as nothing at all (a Form, a Jamboard, a Site) is a
//! refusal with its own code, not a download of an empty file.

/// What a Workspace document is exported as.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Export {
    /// The `mimeType` the export endpoint is asked for.
    pub mime: &'static str,
    /// The extension appended to the document's name, without the dot.
    pub extension: &'static str,
}

/// Why a document cannot be exported.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Refusal {
    /// The document type has no export at all.
    UnsupportedType,
    /// The type exports, but not as the format that was asked for.
    UnsupportedFormat,
}

/// Every format Drive exports to, and the extension each one gets.
const FORMATS: [(&str, &str, &str); 17] = [
    ("pdf", "application/pdf", "pdf"),
    (
        "docx",
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "docx",
    ),
    ("odt", "application/vnd.oasis.opendocument.text", "odt"),
    ("rtf", "application/rtf", "rtf"),
    ("txt", "text/plain", "txt"),
    ("html", "text/html", "html"),
    ("epub", "application/epub+zip", "epub"),
    ("md", "text/markdown", "md"),
    (
        "xlsx",
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "xlsx",
    ),
    (
        "ods",
        "application/vnd.oasis.opendocument.spreadsheet",
        "ods",
    ),
    ("csv", "text/csv", "csv"),
    ("tsv", "text/tab-separated-values", "tsv"),
    (
        "pptx",
        "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "pptx",
    ),
    (
        "odp",
        "application/vnd.oasis.opendocument.presentation",
        "odp",
    ),
    ("png", "image/png", "png"),
    ("jpeg", "image/jpeg", "jpg"),
    ("svg", "image/svg+xml", "svg"),
];

/// `(Drive mime type, default format, the formats this type exports to)`.
///
/// The defaults are the round-trip ones — a Doc comes back as a Word document, a Sheet as an
/// Excel workbook — because the common reason to download a Workspace document is to keep
/// editing it somewhere else. Somebody who wants a PDF says so in the address.
const TYPES: [(&str, &str, &[&str]); 5] = [
    (
        "application/vnd.google-apps.document",
        "docx",
        &["docx", "odt", "rtf", "txt", "html", "epub", "md", "pdf"],
    ),
    (
        "application/vnd.google-apps.spreadsheet",
        "xlsx",
        &["xlsx", "ods", "csv", "tsv", "html", "pdf"],
    ),
    (
        "application/vnd.google-apps.presentation",
        "pptx",
        &["pptx", "odp", "txt", "pdf"],
    ),
    (
        "application/vnd.google-apps.drawing",
        "png",
        &["png", "jpeg", "svg", "pdf"],
    ),
    ("application/vnd.google-apps.script", "json", &["json"]),
];

/// Whether a Drive `mimeType` is a Workspace document rather than an ordinary file.
#[must_use]
pub fn is_workspace_document(mime: &str) -> bool {
    mime.starts_with("application/vnd.google-apps.")
}

/// Whether a Drive `mimeType` is a folder.
#[must_use]
pub fn is_folder(mime: &str) -> bool {
    mime == "application/vnd.google-apps.folder"
}

/// Looks a format word up.
fn format(name: &str) -> Option<Export> {
    // Apps Script is the one export whose mime type is not in the general table, because it is
    // a Google type rather than an interchange format.
    if name == "json" {
        return Some(Export {
            mime: "application/vnd.google-apps.script+json",
            extension: "json",
        });
    }
    FORMATS
        .iter()
        .find(|(word, _, _)| *word == name)
        .map(|(_, mime, extension)| Export { mime, extension })
}

/// What this document becomes, given what the address asked for.
///
/// # Errors
///
/// [`Refusal::UnsupportedType`] for a document type Drive exports nothing for, and
/// [`Refusal::UnsupportedFormat`] when the address named a format this type does not offer.
/// Both are refusals rather than a silent fallback: a Form quietly downloaded as an empty PDF
/// is worse than a message saying Forms cannot be downloaded.
pub fn resolve(mime: &str, requested: Option<&str>) -> Result<Export, Refusal> {
    let (_, default, allowed) = TYPES
        .iter()
        .find(|(known, _, _)| *known == mime)
        .ok_or(Refusal::UnsupportedType)?;
    let wanted = requested.unwrap_or(default);
    if !allowed.contains(&wanted) {
        return Err(Refusal::UnsupportedFormat);
    }
    format(wanted).ok_or(Refusal::UnsupportedFormat)
}

/// The name a Workspace export is saved under: the document's own name plus the extension the
/// chosen format brings, unless the name already ends in it.
#[must_use]
pub fn export_name(name: &str, export: &Export) -> String {
    let suffix = format!(".{}", export.extension);
    if name.to_ascii_lowercase().ends_with(&suffix) {
        return name.to_owned();
    }
    format!("{name}{suffix}")
}

#[cfg(test)]
mod tests {
    use super::{Export, Refusal, export_name, is_folder, is_workspace_document, resolve};

    #[test]
    fn each_document_type_has_a_round_trip_default() {
        assert_eq!(
            resolve("application/vnd.google-apps.document", None),
            Ok(Export {
                mime: "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
                extension: "docx",
            })
        );
        assert_eq!(
            resolve("application/vnd.google-apps.spreadsheet", None).map(|e| e.extension),
            Ok("xlsx")
        );
        assert_eq!(
            resolve("application/vnd.google-apps.presentation", None).map(|e| e.extension),
            Ok("pptx")
        );
        assert_eq!(
            resolve("application/vnd.google-apps.drawing", None).map(|e| e.extension),
            Ok("png")
        );
        assert_eq!(
            resolve("application/vnd.google-apps.script", None).map(|e| e.mime),
            Ok("application/vnd.google-apps.script+json")
        );
    }

    #[test]
    fn an_address_that_asked_for_a_format_gets_it() {
        assert_eq!(
            resolve("application/vnd.google-apps.document", Some("pdf")),
            Ok(Export {
                mime: "application/pdf",
                extension: "pdf",
            })
        );
        assert_eq!(
            resolve("application/vnd.google-apps.spreadsheet", Some("csv")).map(|e| e.extension),
            Ok("csv")
        );
    }

    /// A format one type offers and another does not is refused rather than substituted, so
    /// nobody is handed a file in a format they did not ask for and cannot open.
    #[test]
    fn a_format_this_type_does_not_offer_is_refused() {
        assert_eq!(
            resolve("application/vnd.google-apps.spreadsheet", Some("epub")),
            Err(Refusal::UnsupportedFormat)
        );
        assert_eq!(
            resolve("application/vnd.google-apps.document", Some("png")),
            Err(Refusal::UnsupportedFormat)
        );
    }

    /// A Form, a Jamboard and a Site have no export at all, and say so.
    #[test]
    fn a_type_with_no_export_is_a_refusal_and_not_an_empty_file() {
        for mime in [
            "application/vnd.google-apps.form",
            "application/vnd.google-apps.jam",
            "application/vnd.google-apps.site",
            "application/vnd.google-apps.shortcut",
        ] {
            assert_eq!(resolve(mime, None), Err(Refusal::UnsupportedType), "{mime}");
            assert!(is_workspace_document(mime));
        }
        assert!(is_folder("application/vnd.google-apps.folder"));
        assert!(!is_workspace_document("video/mp4"));
    }

    /// The extension the person will see, which is the whole point of deciding this before
    /// anything is queued.
    #[test]
    fn the_extension_is_part_of_the_name_and_is_never_doubled() {
        let docx = resolve("application/vnd.google-apps.document", None).expect("docx");
        assert_eq!(
            export_name("Quarterly report", &docx),
            "Quarterly report.docx"
        );
        assert_eq!(
            export_name("Quarterly report.docx", &docx),
            "Quarterly report.docx"
        );
        assert_eq!(
            export_name("Quarterly report.DOCX", &docx),
            "Quarterly report.DOCX"
        );
    }
}
