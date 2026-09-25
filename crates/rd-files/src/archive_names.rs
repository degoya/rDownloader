//! Archive volume naming shared by extraction and LinkGrabber grouping.

/// Container family of an archive volume.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ArchiveKind {
    Zip,
    SevenZip,
    Rar,
}

/// One volume of a (possibly multi-part) archive.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArchiveVolume {
    /// Name shared by every volume of the set (without part markers and extension).
    pub base: String,
    pub kind: ArchiveKind,
    /// Zero-based volume index inside the set (`0` = the volume extraction starts from).
    pub index: u32,
    /// Whether extraction must start with this volume.
    pub is_first: bool,
    /// Whether the name carried an explicit volume number (`part01`, `.r00`, `.001`).
    ///
    /// `Release.rar` and `Release.part01.rar` are both index 0 of the same set; only this tells
    /// them apart, and grouping needs that to pick the same entry volume every time (RD-107-11).
    pub numbered: bool,
}

/// Parses `name.rar`, `name.partNN.rar`, `name.rNN`, `name.7z`, `name.7z.NNN`, `name.zip`,
/// `name.zNN` and `name.zip.NNN` (case-insensitive). Returns `None` for non-archives.
#[must_use]
pub fn parse_archive_volume(file_name: &str) -> Option<ArchiveVolume> {
    let lower = file_name.to_ascii_lowercase();
    let (stem, extension) = lower.rsplit_once('.')?;
    if stem.is_empty() {
        return None;
    }
    // Safe against multi-byte names: `to_ascii_lowercase` rewrites only the bytes `A`-`Z`, so
    // `lower` and `file_name` have identical byte lengths and identical char boundaries.
    // RD-107-11 checked this rather than "fixing" it; `recognizes_non_ascii_names` holds it.
    let original_stem = &file_name[..stem.len()];
    match extension {
        "rar" => {
            if let Some((base, part)) = stem.rsplit_once(".part")
                && !base.is_empty()
                && let Ok(number) = part.parse::<u32>()
            {
                return Some(ArchiveVolume {
                    base: file_name[..base.len()].to_owned(),
                    kind: ArchiveKind::Rar,
                    index: number.saturating_sub(1),
                    is_first: number <= 1,
                    numbered: true,
                });
            }
            Some(ArchiveVolume {
                base: original_stem.to_owned(),
                kind: ArchiveKind::Rar,
                index: 0,
                is_first: true,
                numbered: false,
            })
        }
        "7z" => Some(ArchiveVolume {
            base: original_stem.to_owned(),
            kind: ArchiveKind::SevenZip,
            index: 0,
            is_first: true,
            numbered: false,
        }),
        "zip" => Some(ArchiveVolume {
            base: original_stem.to_owned(),
            kind: ArchiveKind::Zip,
            index: 0,
            is_first: true,
            numbered: false,
        }),
        _ => {
            // Old-style RAR volumes: name.r00, name.r01, … (name.rar is the first volume).
            if let Some(digits) = extension.strip_prefix('r')
                && !digits.is_empty()
                && digits.chars().all(|c| c.is_ascii_digit())
            {
                let number: u32 = digits.parse().ok()?;
                return Some(ArchiveVolume {
                    base: original_stem.to_owned(),
                    kind: ArchiveKind::Rar,
                    index: number.saturating_add(1),
                    is_first: false,
                    numbered: true,
                });
            }
            // Split ZIP: name.z01, name.z02, … (name.zip is the last volume but the entry point).
            if let Some(digits) = extension.strip_prefix('z')
                && !digits.is_empty()
                && digits.chars().all(|c| c.is_ascii_digit())
            {
                let number: u32 = digits.parse().ok()?;
                return Some(ArchiveVolume {
                    base: original_stem.to_owned(),
                    kind: ArchiveKind::Zip,
                    index: number,
                    is_first: false,
                    numbered: true,
                });
            }
            // Numeric suffix volumes: name.7z.001 / name.zip.001.
            if extension.len() == 3 && extension.chars().all(|c| c.is_ascii_digit()) {
                let number: u32 = extension.parse().ok()?;
                let (inner_stem, inner_extension) = stem.rsplit_once('.')?;
                let kind = match inner_extension {
                    "7z" => ArchiveKind::SevenZip,
                    "zip" => ArchiveKind::Zip,
                    _ => return None,
                };
                return Some(ArchiveVolume {
                    base: file_name[..inner_stem.len()].to_owned(),
                    kind,
                    index: number.saturating_sub(1),
                    is_first: number <= 1,
                    numbered: true,
                });
            }
            None
        }
    }
}

/// Splits the SABnzbd password convention `release{{secret}}.nzb` into
/// (`release.nzb`, `Some("secret")`). Names without a marker are returned unchanged.
///
/// The secret is taken **verbatim**: the braces already delimit it, so trimming here would be a
/// second, invisible place where a password loses characters. Whitespace is decided once, in
/// `rd_postprocess::password_candidates` (RD-107-11). The cleaned name is still trimmed — that is
/// a file name, not a secret.
#[must_use]
pub fn strip_password_marker(name: &str) -> (String, Option<String>) {
    let Some(start) = name.find("{{") else {
        return (name.to_owned(), None);
    };
    let Some(end) = name.rfind("}}") else {
        return (name.to_owned(), None);
    };
    if end < start + 2 {
        return (name.to_owned(), None);
    }
    let password = &name[start + 2..end];
    if password.is_empty() {
        return (name.to_owned(), None);
    }
    let cleaned = format!("{}{}", &name[..start], &name[end + 2..])
        .trim()
        .to_owned();
    (cleaned, Some(password.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::{ArchiveKind, parse_archive_volume, strip_password_marker};

    #[test]
    fn recognizes_rar_volume_families() {
        let first = parse_archive_volume("Release.part1.rar").expect("part1");
        assert_eq!(first.base, "Release");
        assert_eq!(first.kind, ArchiveKind::Rar);
        assert!(first.is_first);
        let second = parse_archive_volume("Release.part02.rar").expect("part02");
        assert_eq!(second.index, 1);
        assert!(!second.is_first);
        let old = parse_archive_volume("Release.r00").expect("r00");
        assert_eq!(old.index, 1);
        assert!(!old.is_first);
        assert_eq!(old.base, "Release");
        assert!(parse_archive_volume("Release.rar").expect("plain").is_first);
        assert_eq!(
            parse_archive_volume("Release.part001.rar")
                .expect("part001")
                .index,
            0
        );
    }

    #[test]
    fn recognizes_seven_zip_and_zip_volumes() {
        let single = parse_archive_volume("data.7Z").expect("7z");
        assert_eq!(single.kind, ArchiveKind::SevenZip);
        assert_eq!(single.base, "data");
        let split = parse_archive_volume("data.7z.002").expect("7z.002");
        assert_eq!(split.index, 1);
        assert!(!split.is_first);
        assert_eq!(split.base, "data");
        let zip = parse_archive_volume("pack.zip").expect("zip");
        assert!(zip.is_first);
        let z01 = parse_archive_volume("pack.z01").expect("z01");
        assert_eq!(z01.kind, ArchiveKind::Zip);
        assert!(!z01.is_first);
    }

    #[test]
    fn recognizes_non_ascii_names_without_losing_or_splitting_bytes() {
        // RD-107-11 suspected `&file_name[..stem.len()]` of slicing a multi-byte name at the
        // wrong index. It cannot: `to_ascii_lowercase` rewrites only `A`-`Z`, so the byte length
        // is unchanged. This holds that, and would panic rather than drift if it ever changed.
        let volume = parse_archive_volume("\u{fc}ber-Release.RAR").expect("umlaut rar");
        assert_eq!(volume.base, "\u{fc}ber-Release");
        assert!(volume.is_first);
        assert!(!volume.numbered);
        let part = parse_archive_volume("\u{2764}\u{fe0f}Release.PART02.rar").expect("emoji part");
        assert_eq!(part.base, "\u{2764}\u{fe0f}Release");
        assert_eq!(part.index, 1);
        assert!(part.numbered);
        let split = parse_archive_volume("\u{4e2d}\u{6587}.7Z.001").expect("cjk split");
        assert_eq!(split.base, "\u{4e2d}\u{6587}");
        assert_eq!(split.kind, ArchiveKind::SevenZip);
        assert!(split.numbered);
    }

    #[test]
    fn a_marked_password_keeps_its_whitespace() {
        // RD-107-11: the braces delimit the secret, so trimming here would silently change it.
        assert_eq!(
            strip_password_marker("Release{{ s3cret }}.nzb"),
            ("Release.nzb".to_owned(), Some(" s3cret ".to_owned()))
        );
    }

    #[test]
    fn ignores_non_archives() {
        assert!(parse_archive_volume("movie.mkv").is_none());
        assert!(parse_archive_volume("readme").is_none());
        assert!(parse_archive_volume(".rar").is_none());
        assert!(parse_archive_volume("file.r").is_none());
        assert!(parse_archive_volume("file.001").is_none());
    }

    #[test]
    fn strips_password_markers() {
        assert_eq!(
            strip_password_marker("Release{{s3cret}}.nzb"),
            ("Release.nzb".to_owned(), Some("s3cret".to_owned()))
        );
        assert_eq!(
            strip_password_marker("Release{{a{b}c}}.nzb"),
            ("Release.nzb".to_owned(), Some("a{b}c".to_owned()))
        );
        assert_eq!(
            strip_password_marker("Release.nzb"),
            ("Release.nzb".to_owned(), None)
        );
        assert_eq!(
            strip_password_marker("Release{{}}.nzb"),
            ("Release{{}}.nzb".to_owned(), None)
        );
    }
}
