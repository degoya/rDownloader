//! The NZBs inside a ZIP, as an indexer's cart hands them out (RD-130-16).
//!
//! NNTmux answers `/getnzb?id=…&zip=1` with one ZIP holding an NZB per cart entry. Nothing took
//! such a file before: the hotfolder refuses `.zip` outright, and the LinkGrabber saw an archive
//! it would have downloaded as a file. This reads the archive in memory and hands back the NZB
//! members, and nothing else.
//!
//! An archive is a decompression bomb until proven otherwise, so every figure it declares is a
//! claim and not a fact: members are read through a `take` one byte past their limit, and the
//! sum of what was actually read is what counts against the total.

use std::io::{Cursor, Read};

use crate::ApiError;

/// What an archive may hold. [`ZipLimits::DEFAULT`] in the service; the tests pass smaller ones
/// so a bomb does not have to be sixty-four real megabytes to be one.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ZipLimits {
    /// The most members an archive may list, NZB or not.
    pub members: usize,
    /// The most one NZB member may inflate to.
    pub member_bytes: usize,
    /// The most all NZB members together may inflate to.
    pub total_bytes: usize,
}

impl ZipLimits {
    /// A cart of a few hundred releases, each NZB within the limit an uploaded NZB has.
    pub(crate) const DEFAULT: Self = Self {
        members: 512,
        member_bytes: rd_collector::MAX_NZB_BYTES,
        total_bytes: 256 * 1024 * 1024,
    };
}

/// The four bytes every ZIP with at least one member starts with.
const LOCAL_HEADER: &[u8] = b"PK\x03\x04";

/// The signature of an archive without members (only the end-of-directory record).
const EMPTY_ARCHIVE: &[u8] = b"PK\x05\x06";

/// One NZB taken out of an archive: the base name it had in there, and its bytes.
#[derive(Debug)]
pub(crate) struct ZipMember {
    pub file_name: String,
    pub bytes: Vec<u8>,
}

/// Whether these bytes are a ZIP archive at all, by signature.
pub(crate) fn is_zip(bytes: &[u8]) -> bool {
    bytes.starts_with(LOCAL_HEADER) || bytes.starts_with(EMPTY_ARCHIVE)
}

/// The NZB members of an archive, in archive order.
///
/// Directories, other files and the `__MACOSX/` resource forks a Mac adds are passed over; an
/// archive without a single NZB is refused under `capture.zip_without_nzb`. An NZB member that
/// is encrypted, damaged or over a limit refuses the whole archive under `capture.zip_invalid`:
/// half a cart imported and the other half left behind is worse than none, because the person
/// then cannot tell which half is missing.
pub(crate) fn nzb_members(bytes: &[u8], limits: ZipLimits) -> Result<Vec<ZipMember>, ApiError> {
    let mut archive =
        zip::ZipArchive::new(Cursor::new(bytes)).map_err(|_| zip_invalid("unreadable"))?;
    if archive.len() > limits.members {
        return Err(zip_invalid("too_many_members").with_param("max_members", limits.members));
    }
    let mut members = Vec::new();
    let mut total = 0_usize;
    for index in 0..archive.len() {
        // The raw entry answers the questions about a member without inflating or decrypting
        // it, so an encrypted readme beside the NZBs does not refuse the cart.
        let (file_name, encrypted) = {
            let raw = archive
                .by_index_raw(index)
                .map_err(|_| zip_invalid("unreadable_member"))?;
            let path = raw.name().replace('\\', "/");
            if raw.is_dir() || path.starts_with("__MACOSX/") {
                continue;
            }
            let file_name = path.rsplit('/').next().unwrap_or_default().to_owned();
            if !is_nzb_name(&file_name) {
                continue;
            }
            (file_name, raw.encrypted())
        };
        if encrypted {
            return Err(zip_invalid("encrypted"));
        }
        let member = archive
            .by_index(index)
            .map_err(|_| zip_invalid("unreadable_member"))?;
        let mut content = Vec::new();
        member
            .take(limits.member_bytes as u64 + 1)
            .read_to_end(&mut content)
            .map_err(|_| zip_invalid("unreadable_member"))?;
        if content.len() > limits.member_bytes {
            return Err(
                zip_invalid("member_too_large").with_param("max_bytes", limits.member_bytes)
            );
        }
        total += content.len();
        if total > limits.total_bytes {
            return Err(zip_invalid("too_large").with_param("max_bytes", limits.total_bytes));
        }
        members.push(ZipMember {
            file_name,
            bytes: content,
        });
    }
    if members.is_empty() {
        return Err(ApiError::bad_request(
            "capture.zip_without_nzb",
            "The ZIP archive contains no NZB file",
        ));
    }
    Ok(members)
}

fn is_nzb_name(name: &str) -> bool {
    name.len() > ".nzb".len() && name.to_ascii_lowercase().ends_with(".nzb")
}

fn zip_invalid(reason: &str) -> ApiError {
    ApiError::bad_request("capture.zip_invalid", "The ZIP archive cannot be imported")
        .with_param("reason", reason)
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Write};

    use zip::write::SimpleFileOptions;

    use super::{ZipLimits, is_zip, nzb_members};

    const SMALL: ZipLimits = ZipLimits {
        members: 8,
        member_bytes: 64,
        total_bytes: 100,
    };

    fn archive(members: &[(&str, &[u8])]) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, content) in members {
            if name.ends_with('/') {
                writer
                    .add_directory(*name, SimpleFileOptions::default())
                    .expect("directory");
                continue;
            }
            writer
                .start_file(*name, SimpleFileOptions::default())
                .expect("member");
            writer.write_all(content).expect("content");
        }
        writer.finish().expect("finish").into_inner()
    }

    #[test]
    fn the_nzb_members_come_out_by_base_name_and_nothing_else_does() {
        let bytes = archive(&[
            ("cart/", b""),
            ("cart/First.Release.nzb", b"<nzb/>"),
            ("cart/readme.txt", b"hello"),
            ("__MACOSX/cart/._First.Release.nzb", b"fork"),
            ("Second.NZB", b"<nzb></nzb>"),
        ]);
        assert!(is_zip(&bytes));
        let members = nzb_members(&bytes, ZipLimits::DEFAULT).expect("members");
        let names: Vec<&str> = members
            .iter()
            .map(|member| member.file_name.as_str())
            .collect();
        assert_eq!(names, ["First.Release.nzb", "Second.NZB"]);
        assert_eq!(members[0].bytes, b"<nzb/>");
    }

    #[test]
    fn an_archive_without_an_nzb_says_so() {
        let error = nzb_members(&archive(&[("notes.txt", b"x")]), SMALL).expect_err("refused");
        assert_eq!(error.code(), "capture.zip_without_nzb");
    }

    #[test]
    fn what_is_not_an_archive_is_refused_not_guessed_at() {
        assert!(!is_zip(b"<nzb/>"));
        let error = nzb_members(b"PK\x03\x04 but nothing after", SMALL).expect_err("refused");
        assert_eq!(error.code(), "capture.zip_invalid");
    }

    /// Declared sizes are claims; a member is read one byte past the limit, and that byte is
    /// what refuses it. A highly compressible member is the bomb's shape.
    #[test]
    fn a_member_over_the_limit_refuses_the_whole_archive() {
        let big = vec![b'a'; SMALL.member_bytes + 1];
        let bytes = archive(&[("ok.nzb", b"<nzb/>"), ("big.nzb", &big)]);
        let error = nzb_members(&bytes, SMALL).expect_err("refused");
        assert_eq!(error.code(), "capture.zip_invalid");
        let at = vec![b'a'; SMALL.member_bytes];
        assert_eq!(
            nzb_members(&archive(&[("at.nzb", &at)]), SMALL)
                .expect("at the limit")
                .len(),
            1
        );
    }

    /// Each member within its own limit, the sum over the archive's.
    #[test]
    fn the_members_together_are_held_to_the_total() {
        let part = vec![b'a'; 60];
        let bytes = archive(&[("one.nzb", &part), ("two.nzb", &part)]);
        let error = nzb_members(&bytes, SMALL).expect_err("refused");
        assert_eq!(error.code(), "capture.zip_invalid");
    }

    #[test]
    fn an_archive_listing_too_many_members_is_refused_before_any_is_read() {
        let names: Vec<String> = (0..=SMALL.members)
            .map(|index| format!("{index}.txt"))
            .collect();
        let members: Vec<(&str, &[u8])> =
            names.iter().map(|name| (name.as_str(), &b""[..])).collect();
        let error = nzb_members(&archive(&members), SMALL).expect_err("refused");
        assert_eq!(error.code(), "capture.zip_invalid");
    }
}
