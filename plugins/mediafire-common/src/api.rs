//! Reading the documents MediaFire's API answers with.
//!
//! Every call answers `{"response": {...}}` with `result` set to `Success` or `Error`, and
//! an error carries a numeric `error` and a `message`. The status code is not the signal:
//! an unknown key is a `404` today and was a `400` on 2026-09-21 00:30, both with the same
//! envelope, so the envelope is read first and the status only where there is none.
//! Numbers arrive as strings (`"size": "10485760"`), which [`number`] tolerates.

use serde_json::Value;

/// `ERROR_RATE_LIMIT`: "Maximum number of allowed calls for this resource is reached in a
/// specified time".
pub const ERROR_RATE_LIMIT: u32 = 261;
/// "Session Token is missing" — also what an unknown *folder* key is reported as.
pub const ERROR_TOKEN_MISSING: u32 = 104;
/// "Session Token is invalid".
pub const ERROR_TOKEN_INVALID: u32 = 105;
/// "Unknown or Invalid QuickKey".
pub const ERROR_QUICKKEY_UNKNOWN: u32 = 110;
/// "Quick Key is missing" — also what a key of the wrong length, or a folder key, gets.
pub const ERROR_QUICKKEY_MISSING: u32 = 111;
/// "Unknown or Invalid FolderKey".
pub const ERROR_FOLDERKEY_UNKNOWN: u32 = 112;
/// "Folder Key is missing".
pub const ERROR_FOLDERKEY_MISSING: u32 = 113;
/// "Access denied".
pub const ERROR_ACCESS_DENIED: u32 = 114;

/// The longest provider message that is repeated back; the rest is cut.
const MAX_MESSAGE_CHARS: usize = 120;

/// One API error, as the envelope names it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ApiError {
    pub code: u32,
    /// The provider's text, sanitised: printable, single-line, bounded.
    pub message: String,
}

/// What an envelope held.
#[derive(Clone, Debug, PartialEq)]
pub enum Envelope {
    /// The `response` object of a successful call.
    Success(Value),
    Error(ApiError),
}

/// Reads an envelope, or `None` when the bytes are not one.
#[must_use]
pub fn envelope(body: &[u8]) -> Option<Envelope> {
    let document: Value = serde_json::from_slice(body).ok()?;
    let response = document.get("response")?.as_object()?;
    match response.get("result").and_then(Value::as_str) {
        Some("Success") => Some(Envelope::Success(Value::Object(response.clone()))),
        Some("Error") => Some(Envelope::Error(ApiError {
            code: response
                .get("error")
                .and_then(number)
                .and_then(|code| u32::try_from(code).ok())?,
            message: sanitize(
                response
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            ),
        })),
        _ => None,
    }
}

/// What the API knows about one file.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileInfo {
    pub key: String,
    pub name: String,
    pub size: Option<u64>,
    /// SHA-256 of the content, as the API reports it.
    pub hash: Option<String>,
    pub private: bool,
    pub password_protected: bool,
    /// `ready: "no"` is an upload still being processed.
    pub ready: bool,
}

/// Reads one `file_info` object.
#[must_use]
pub fn file_info(value: &Value) -> Option<FileInfo> {
    let key = value.get("quickkey").and_then(Value::as_str)?;
    Some(FileInfo {
        key: key.to_owned(),
        name: value
            .get("filename")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        size: value.get("size").and_then(number),
        hash: value
            .get("hash")
            .and_then(Value::as_str)
            .filter(|hash| hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit()))
            .map(str::to_ascii_lowercase),
        private: text(value, "privacy") == "private",
        password_protected: yes(value, "password_protected"),
        ready: text(value, "ready") != "no",
    })
}

/// The files a `file/get_info` answer carries: `file_info` for one key, `file_infos[]` for
/// several. Keys the API skipped are simply absent.
#[must_use]
pub fn file_infos(response: &Value) -> Vec<FileInfo> {
    if let Some(one) = response.get("file_info") {
        return file_info(one).into_iter().collect();
    }
    response
        .get("file_infos")
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(file_info).collect())
        .unwrap_or_default()
}

/// What the API knows about one folder.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FolderInfo {
    pub key: String,
    pub name: String,
    pub private: bool,
    pub file_count: Option<u64>,
    pub folder_count: Option<u64>,
}

/// Reads one folder object — `folder_info` of `folder/get_info`, or an entry of
/// `folder_content.folders`.
#[must_use]
pub fn folder_info(value: &Value) -> Option<FolderInfo> {
    let key = value.get("folderkey").and_then(Value::as_str)?;
    Some(FolderInfo {
        key: key.to_owned(),
        name: value
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        private: text(value, "privacy") == "private",
        file_count: value.get("file_count").and_then(number),
        folder_count: value.get("folder_count").and_then(number),
    })
}

/// One chunk of a folder's content.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FolderContent {
    pub files: Vec<FileInfo>,
    pub folders: Vec<FolderInfo>,
    /// Whether `chunk + 1` exists.
    pub more_chunks: bool,
}

/// Reads a `folder/get_content` answer; either list may be absent, depending on the
/// `content_type` that was asked for.
#[must_use]
pub fn folder_content(response: &Value) -> Option<FolderContent> {
    let content = response.get("folder_content")?;
    let list = |name: &str| content.get(name).and_then(Value::as_array);
    Some(FolderContent {
        files: list("files")
            .map(|items| items.iter().filter_map(file_info).collect())
            .unwrap_or_default(),
        folders: list("folders")
            .map(|items| items.iter().filter_map(folder_info).collect())
            .unwrap_or_default(),
        more_chunks: yes(content, "more_chunks"),
    })
}

/// A number the API wrote either as a number or as a string of digits.
#[must_use]
pub fn number(value: &Value) -> Option<u64> {
    match value {
        Value::Number(number) => number.as_u64(),
        Value::String(text) => text.trim().parse().ok(),
        _ => None,
    }
}

/// A provider text fit for a message: control characters dropped, whitespace collapsed,
/// cut to [`MAX_MESSAGE_CHARS`].
#[must_use]
pub fn sanitize(message: &str) -> String {
    let mut clean = String::with_capacity(message.len().min(MAX_MESSAGE_CHARS));
    let mut space = true;
    for character in message
        .chars()
        .filter(|character| character.is_whitespace() || !character.is_control())
    {
        if character.is_whitespace() {
            if !space {
                clean.push(' ');
            }
            space = true;
        } else {
            clean.push(character);
            space = false;
        }
        if clean.chars().count() >= MAX_MESSAGE_CHARS {
            break;
        }
    }
    clean.trim_end().to_owned()
}

fn text<'a>(value: &'a Value, name: &str) -> &'a str {
    value.get(name).and_then(Value::as_str).unwrap_or_default()
}

fn yes(value: &Value, name: &str) -> bool {
    text(value, name).eq_ignore_ascii_case("yes")
}

#[cfg(test)]
mod tests {
    use super::{ApiError, Envelope, envelope, file_infos, folder_content, folder_info, sanitize};

    #[test]
    fn a_success_envelope_hands_out_the_response_and_an_error_its_code() {
        let success = envelope(
            br#"{"response":{"action":"file/get_info","file_info":{"quickkey":"ipnyzofjcwri357","filename":"test-10mb.bin","size":"10485760","hash":"e5b844cc57f57094ea4585e235f36c78c1cd222262bb89d53c94dcb4d6b3e55d","privacy":"public","password_protected":"no","ready":"yes"},"result":"Success","current_api_version":"1.5"}}"#,
        );
        let Some(Envelope::Success(response)) = success else {
            panic!("expected success: {success:?}");
        };
        let files = file_infos(&response);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].name, "test-10mb.bin");
        assert_eq!(files[0].size, Some(10_485_760));
        assert_eq!(
            files[0].hash.as_deref(),
            Some("e5b844cc57f57094ea4585e235f36c78c1cd222262bb89d53c94dcb4d6b3e55d")
        );
        assert!(!files[0].private && !files[0].password_protected && files[0].ready);

        assert_eq!(
            envelope(
                br#"{"response":{"action":"file/get_info","message":"Unknown or Invalid QuickKey","error":110,"result":"Error","current_api_version":"1.5"}}"#
            ),
            Some(Envelope::Error(ApiError {
                code: 110,
                message: "Unknown or Invalid QuickKey".to_owned()
            }))
        );
        assert_eq!(envelope(b"<html>"), None);
        assert_eq!(envelope(br#"{"response":{"result":"Maybe"}}"#), None);
        assert_eq!(envelope(br#"{"response":{"result":"Error"}}"#), None);
    }

    #[test]
    fn a_batch_lists_the_found_files_and_leaves_the_skipped_key_out() {
        let Some(Envelope::Success(response)) = envelope(
            br#"{"response":{"file_infos":[{"quickkey":"a","filename":"a.bin","size":"1"},{"filename":"no key"},{"quickkey":"b","filename":"b.bin","privacy":"private","password_protected":"YES","ready":"no"}],"skipped":"c","result":"Success"}}"#,
        ) else {
            panic!("success")
        };
        let files = file_infos(&response);
        assert_eq!(files.len(), 2);
        assert_eq!(files[1].key, "b");
        assert!(files[1].private && files[1].password_protected && !files[1].ready);
        assert_eq!(files[1].size, None);
    }

    #[test]
    fn a_folder_and_its_content_are_read_with_their_counts() {
        let Some(Envelope::Success(response)) = envelope(
            br#"{"response":{"folder_info":{"folderkey":"rww7bhhi0yc1l","name":"Walls","privacy":"public","file_count":"19","folder_count":2},"result":"Success"}}"#,
        ) else {
            panic!("success")
        };
        let info = folder_info(response.get("folder_info").expect("folder_info")).expect("info");
        assert_eq!(info.name, "Walls");
        assert_eq!((info.file_count, info.folder_count), (Some(19), Some(2)));

        let Some(Envelope::Success(response)) = envelope(
            br#"{"response":{"folder_content":{"chunk_size":"100","content_type":"folders","chunk_number":"1","folders":[{"folderkey":"x67lis2zygml5","name":"Empty","privacy":"public"}],"more_chunks":"yes"},"result":"Success"}}"#,
        ) else {
            panic!("success")
        };
        let content = folder_content(&response).expect("content");
        assert!(content.files.is_empty());
        assert_eq!(content.folders[0].name, "Empty");
        assert!(content.more_chunks);
    }

    #[test]
    fn provider_text_is_bounded_and_single_line() {
        assert_eq!(
            sanitize("  Quick\tKey\n is\x00 missing  "),
            "Quick Key is missing"
        );
        assert_eq!(sanitize(&"x".repeat(500)).chars().count(), 120);
        assert_eq!(sanitize(""), "");
    }
}
