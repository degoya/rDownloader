//! Nothing that could be a credential is committed with the contract fixtures: one table, one
//! row per fixture directory, one test (RD-1280-05; it replaces seventeen copies of the same
//! loop, one per module). A row names its directory, how many documents it holds, and the rule
//! its documents keep. The rules are the modules' own, moved here unchanged.

use std::path::Path;

use crate::support::storage::sign_in::{
    PLACEHOLDER_ACCESS_TOKEN as ACCESS, PLACEHOLDER_REFRESH_TOKEN as REFRESH,
};

mod text;

use text::Text;

const DEVICE: &str = "redacted-device-code-0000";
const CLIENT_ID: &str = "redacted-client-id-0000";
const CLIENT_SECRET: &str = "redacted-client-secret-0000";
/// The SHA-1 of nothing, the info hash the remote-job fixtures name.
const HASH: &str = "da39a3ee5e6b4b0d3255bfef95601890afd80709";
/// What a provider's developer-facing sentence is replaced with.
const SENTENCE: &str = "redacted provider sentence";
const TOKENS: &[&str] = &["access_token", "refresh_token", "id_token", "code"];
const DEVICE_TOKENS: &[&str] = &[
    "access_token",
    "refresh_token",
    "id_token",
    "code",
    "device_code",
];
const PUTIO_TOKENS: &[&str] = &[
    "access_token",
    "oauth_token",
    "refresh_token",
    "client_secret",
];
const ZEROES: &str = "0000000000000000000000000000000000000000";
/// The hosts a listing fixture may name: documentation domains, and `.invalid`, which resolves
/// nowhere and is what a hostile listing's off-site link needs to be.
const DOCUMENTATION_HOSTS: &[&str] = &[".example.org", ".example.net", ".invalid"];
/// What no MediaFire capture may carry: a session, a key, an address, a person's name.
const MEDIAFIRE_WORDS: &[&str] = &["session_token", "api_key", "@", "Vlas", "Santiago", "Mansi"];

/// How many documents a directory holds.
pub enum Count {
    AtLeast(usize),
    Exactly(usize),
}

/// What a named field may hold.
pub enum Allowed {
    /// Nothing: the field may not appear at all.
    Absent,
    /// This string and nothing else.
    Exactly(&'static str),
    /// When it is a string: one of these placeholders.
    OneOf(&'static [&'static str]),
    /// When it is a string: one that starts with this.
    Prefix(&'static str),
    /// When it is a string: an address under this prefix with `redacted` in it.
    RedactedAddress(&'static str),
    /// When it is a string: the info hash of nothing, one of `also`, or a run of `filler`.
    Digest {
        also: &'static [&'static str],
        filler: &'static [char],
    },
}

/// What an address in a fixture has to contain, or start with.
pub enum Link {
    Contains(&'static str),
    Prefix(&'static str),
}

/// A JSON document read at every depth: its named fields, every address and every magnet.
pub struct Walk {
    pub fields: &'static [(&'static [&'static str], Allowed)],
    pub links: &'static [Link],
    /// What a magnet has to name, when the directory holds magnets at all.
    pub magnets: Option<&'static [&'static str]>,
}

pub enum Rule {
    /// A JSON document's top-level fields.
    TopLevel(&'static [(&'static [&'static str], Allowed)]),
    Walk(Walk),
    /// A page or a capture read as text.
    Text(Text),
}

pub struct Row {
    /// Relative to this crate.
    pub directory: &'static str,
    /// Only these documents, when the directory holds others a module does not read.
    pub only: &'static [&'static str],
    pub count: Count,
    pub rule: Rule,
}

const fn top(
    directory: &'static str,
    count: usize,
    fields: &'static [(&'static [&'static str], Allowed)],
) -> Row {
    Row {
        directory,
        only: &[],
        count: Count::AtLeast(count),
        rule: Rule::TopLevel(fields),
    }
}

const fn walk(directory: &'static str, count: usize, rule: Walk) -> Row {
    Row {
        directory,
        only: &[],
        count: Count::AtLeast(count),
        rule: Rule::Walk(rule),
    }
}

/// One row per fixture directory, kept one line per rule so the table reads as one.
#[rustfmt::skip]
pub const TABLE: &[Row] = &[
    // `code` is deliberately not Box's: in a Box error document it is the API's own error name.
    top("tests/fixtures/box", 10, &[
        (&["access_token", "refresh_token", "request_id"], Allowed::Prefix("redacted-")),
    ]),
    // A cursor is a stateful value Dropbox issues per account; the fixtures carry its shape.
    top("tests/fixtures/dropbox", 12, &[
        (TOKENS, Allowed::OneOf(&[ACCESS, REFRESH])),
        (&["cursor"], Allowed::Prefix("AAE_redacted")),
    ]),
    top("tests/fixtures/google_drive", 9, &[(TOKENS, Allowed::OneOf(&[ACCESS, REFRESH]))]),
    // A real driveItem's pre-authenticated download address is a credential in its query.
    top("tests/fixtures/onedrive", 13, &[
        (DEVICE_TOKENS, Allowed::OneOf(&[ACCESS, REFRESH, DEVICE])),
        (&["@microsoft.graph.downloadUrl"], Allowed::RedactedAddress("https://example.invalid/")),
    ]),
    // A download ticket is a pre-authorised, per-request path.
    top("tests/fixtures/pcloud", 13, &[
        (&["access_token", "auth", "code", "client_secret"], Allowed::OneOf(&[ACCESS])),
        (&["path"], Allowed::Prefix("/redacted-")),
    ]),
    top("tests/fixtures/oauth", 7, &[(DEVICE_TOKENS, Allowed::OneOf(&[ACCESS, REFRESH, DEVICE]))]),
    top("tests/fixtures/realdebrid", 10, &[(
        &["access_token", "refresh_token", "id_token", "code", "device_code", "client_id",
            "client_secret"],
        Allowed::OneOf(&[ACCESS, REFRESH, DEVICE, CLIENT_ID, CLIENT_SECRET]),
    )]),
    // The one directory that holds a granted exchange, so a token field holds the placeholder.
    walk("tests/fixtures/putio", 8, Walk {
        fields: &[
            (PUTIO_TOKENS, Allowed::Exactly(ACCESS)),
            (&["error_message"], Allowed::Exactly(SENTENCE)),
        ],
        links: &[Link::Contains("invalid"), Link::Prefix("https://api.put.io/")],
        magnets: None,
    }),
    // Put.io's `error_message` is written for a developer and never repeated; a placeholder makes
    // a test asserting that it did not travel assert something.
    walk("tests/fixtures/putio_transfers", 16, Walk {
        fields: &[
            (PUTIO_TOKENS, Allowed::Absent),
            (&["hash"], Allowed::Digest { also: &[], filler: &['f'] }),
            (&["error_message", "status_message"], Allowed::Exactly(SENTENCE)),
        ],
        links: &[Link::Contains("invalid"), Link::Prefix("https://api.put.io/")],
        magnets: None,
    }),
    walk("tests/fixtures/realdebrid_torrents", 10, Walk {
        fields: &[
            (&["access_token", "refresh_token", "token", "client_secret"], Allowed::Absent),
            (&["id"], Allowed::Prefix("REDACTED")),
            (&["hash"], Allowed::Digest { also: &[], filler: &['0'] }),
        ],
        links: &[Link::Contains("REDACTED"), Link::Prefix("https://api.real-debrid.com/")],
        magnets: None,
    }),
    // TorBox states a per-account value beside every job it creates.
    walk("tests/fixtures/torbox_jobs", 18, Walk {
        fields: &[
            (&["access_token", "refresh_token", "token", "api_key", "client_secret"],
                Allowed::Absent),
            (&["auth_id"], Allowed::Prefix("REDACTED")),
            (&["hash"], Allowed::Digest { also: &["{{hash}}"], filler: &['0'] }),
        ],
        links: &[Link::Contains("REDACTED"), Link::Prefix("https://api.torbox.app/")],
        magnets: None,
    }),
    walk("tests/fixtures/premiumize_transfers", 18, Walk {
        fields: &[
            (&["apikey", "api_key", "access_token", "token", "customer_id"], Allowed::Absent),
            (&["id", "folder_id", "file_id"], Allowed::Prefix("REDACTED")),
        ],
        links: &[Link::Contains("REDACTED")],
        magnets: None,
    }),
    // A magnet names the SHA-1 of nothing, in hex or in base32, or a run of zeroes: all
    // recognisable as placeholders, none of them somebody's content.
    walk("tests/fixtures/offcloud_cloud", 12, Walk {
        fields: &[
            (&["apiKey", "api_key", "key", "token", "password", "connect.sid"], Allowed::Absent),
            (&["requestId", "userId"], Allowed::Prefix("REDACTED")),
        ],
        links: &[Link::Contains("REDACTED"), Link::Contains("example.invalid")],
        magnets: Some(&[HASH, "3i42h3s6nnfq2msvx7xzkyaysc", ZEROES]),
    }),
    walk("tests/fixtures/seedr_jobs", 12, Walk {
        fields: &[
            (&["api_key", "key", "token", "password", "email", "username"], Allowed::Absent),
            (&["torrent_hash"], Allowed::Digest { also: &[], filler: &['f', 'F', '0'] }),
        ],
        links: &[Link::Contains("example.invalid")],
        magnets: None,
    }),
    // A share token and a request token may only be the invented ones; sabre/dav's own 401
    // names the `Authorization: Basic` header, so what must not follow it is an encoded value.
    Row {
        directory: "tests/fixtures/nextcloud_crawler",
        only: &[],
        count: Count::Exactly(12),
        rule: Rule::Text(Text {
            absent: &[],
            host_suffixes: DOCUMENTATION_HOSTS,
            // The schema namespaces a WebDAV document declares: identifiers, never requested.
            hosts: &["sabredav.org", "nextcloud.org", "owncloud.org"],
            tokens: &[("/s/", text::SHARE_TOKENS), ("/dav/files/", text::SHARE_TOKENS)],
            attributes: &[("data-requesttoken=\"", text::REQUEST_TOKENS)],
            encoded_after: &["Basic ", "Bearer "],
        }),
    },
    // An open listing carries no credential at all, and no fixture of one may either.
    Row {
        directory: "tests/fixtures/directory_index_crawler",
        only: &[],
        count: Count::Exactly(12),
        rule: Rule::Text(Text {
            absent: &["Basic ", "Bearer ", "Authorization:", "Set-Cookie"],
            host_suffixes: DOCUMENTATION_HOSTS,
            hosts: &[],
            tokens: &[],
            attributes: &[],
            encoded_after: &[],
        }),
    },
    // The sanitised MediaFire captures the crawler's contract module reads, in two plugins.
    Row {
        directory: "../../plugins/mediafire-crawler/tests/fixtures",
        only: &[
            "api-folder-get-info-2026-09-21.json",
            "api-folder-get-content-files-2026-09-21.json",
            "api-folder-get-content-folders-2026-09-21.json",
            "api-folder-get-info-missing-token-2026-09-21.json",
            "api-file-get-info-folderkey-2026-09-21.json",
        ],
        count: Count::Exactly(5),
        rule: Rule::Text(Text::absent(MEDIAFIRE_WORDS)),
    },
    Row {
        directory: "../../plugins/mediafire/tests/fixtures",
        only: &["api-file-get-info-batch-2026-09-21.json"],
        count: Count::Exactly(1),
        rule: Rule::Text(Text::absent(MEDIAFIRE_WORDS)),
    },
];

fn holds(path: &Path, name: &str, value: Option<&serde_json::Value>, allowed: &Allowed) {
    let text = value.and_then(serde_json::Value::as_str);
    match allowed {
        Allowed::Absent => assert!(value.is_none(), "{path:?} carries a `{name}` field"),
        Allowed::Exactly(expected) => {
            assert_eq!(text, Some(*expected), "{path:?} carries a real `{name}`");
        }
        _ => {
            let Some(found) = text else { return };
            let kept = match allowed {
                Allowed::OneOf(values) => values.contains(&found),
                Allowed::Prefix(prefix) => found.starts_with(*prefix),
                Allowed::RedactedAddress(prefix) => {
                    found.starts_with(*prefix) && found.contains("redacted")
                }
                Allowed::Digest { also, filler } => {
                    found.eq_ignore_ascii_case(HASH)
                        || also.contains(&found)
                        || found.chars().all(|character| filler.contains(&character))
                }
                Allowed::Absent | Allowed::Exactly(_) => true,
            };
            assert!(
                kept,
                "{path:?} carries a `{name}` that is not a placeholder: {found}"
            );
        }
    }
}

fn walk_value(path: &Path, value: &serde_json::Value, rule: &Walk) {
    match value {
        serde_json::Value::Object(fields) => {
            for (name, inner) in fields {
                for (names, allowed) in rule.fields {
                    if names.contains(&name.as_str()) {
                        holds(path, name, Some(inner), allowed);
                    }
                }
                walk_value(path, inner, rule);
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                walk_value(path, item, rule);
            }
        }
        serde_json::Value::String(text) if text.starts_with("http") => {
            let kept = rule.links.iter().any(|link| match link {
                Link::Contains(word) => text.contains(*word),
                Link::Prefix(prefix) => text.starts_with(*prefix),
            });
            assert!(kept, "{path:?} carries a live address: {text}");
        }
        serde_json::Value::String(text) if text.starts_with("magnet:") => {
            if let Some(words) = rule.magnets {
                let lower = text.to_ascii_lowercase();
                assert!(
                    words.iter().any(|word| lower.contains(*word)),
                    "{path:?} carries a real info hash: {text}"
                );
            }
        }
        _ => {}
    }
}

fn check(path: &Path, body: &str, rule: &Rule) {
    let json = || -> serde_json::Value { serde_json::from_str(body).expect("fixture is JSON") };
    match rule {
        Rule::TopLevel(fields) => {
            let value = json();
            for (names, allowed) in *fields {
                for name in *names {
                    holds(path, name, value.get(name), allowed);
                }
            }
        }
        Rule::Walk(rule) => walk_value(path, &json(), rule),
        Rule::Text(rule) => rule.check(path, body),
    }
}

/// Nothing that could be a credential is committed to the repository.
#[test]
fn fixtures_carry_no_credential_material() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    for row in TABLE {
        let directory = root.join(row.directory);
        let paths: Vec<_> = if row.only.is_empty() {
            std::fs::read_dir(&directory)
                .expect("the fixture directory")
                .map(|entry| entry.expect("a fixture").path())
                .collect()
        } else {
            row.only.iter().map(|name| directory.join(name)).collect()
        };
        for path in &paths {
            let body = std::fs::read_to_string(path).expect("a fixture");
            check(path, &body, &row.rule);
        }
        let checked = paths.len();
        match row.count {
            Count::AtLeast(expected) => assert!(
                checked >= expected,
                "{}: only {checked} fixtures were checked",
                row.directory
            ),
            Count::Exactly(expected) => assert_eq!(
                checked, expected,
                "{}: the directory holds {expected} documents",
                row.directory
            ),
        }
    }
}
