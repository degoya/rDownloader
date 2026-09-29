//! Paths written on another machine, and moving them onto this one (RD-160-03).
//!
//! A backup made on Windows names `D:\Downloads\Film` or `\\nas\media\Film`; restored on Linux,
//! `std::path` would read either as one relative file name. So a stored path is read here by its
//! own shape, not by this system's rules: a drive letter or a UNC share is a Windows path with
//! either separator and case-insensitive names, a leading `/` a POSIX one.
//!
//! A remap moves a storage root: every stored path below the root's old place becomes the same
//! relative path below its new one. The relative part has to be plain names only — the rule
//! `rd_files::StorageRoot::resolve` applies to every download — so a stored `..` cannot carry a
//! remapped path out of its root, and a name that means something else on this system (a
//! backslash in a POSIX name restored onto Windows, a drive) is refused rather than reshaped.

use std::path::{Component, Path, PathBuf};

/// Which rules a stored path was written under.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PathStyle {
    Windows,
    Posix,
}

/// An absolute path as another machine wrote it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ForeignPath {
    style: PathStyle,
    /// `c:` for a drive, `\\server\share` for a share, empty for POSIX; lowercase on Windows.
    prefix: String,
    components: Vec<String>,
}

impl ForeignPath {
    /// Reads an absolute path of either system; `None` for a relative or empty one.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        let value = value.trim_end_matches('\0');
        // Verbatim forms first: `\\?\C:\…` is the drive, `\\?\UNC\server\share\…` the share.
        let value = value
            .strip_prefix(r"\\?\UNC\")
            .map(|rest| format!(r"\\{rest}"))
            .or_else(|| value.strip_prefix(r"\\?\").map(str::to_owned))
            .unwrap_or_else(|| value.to_owned());
        let windows_split = |rest: &str| -> Vec<String> {
            rest.split(['\\', '/'])
                .filter(|segment| !segment.is_empty() && *segment != ".")
                .map(str::to_owned)
                .collect()
        };
        if let Some(rest) = value.strip_prefix(r"\\") {
            let mut parts = windows_split(rest).into_iter();
            let (server, share) = (parts.next()?, parts.next()?);
            return Some(Self {
                style: PathStyle::Windows,
                prefix: format!(r"\\{server}\{share}").to_lowercase(),
                components: parts.collect(),
            });
        }
        let bytes = value.as_bytes();
        if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
            // `C:name` is relative to the drive's current folder: not a place.
            if bytes.len() > 2 && !matches!(bytes[2], b'\\' | b'/') {
                return None;
            }
            return Some(Self {
                style: PathStyle::Windows,
                prefix: value[..2].to_lowercase(),
                components: windows_split(&value[2..]),
            });
        }
        if let Some(rest) = value.strip_prefix('/') {
            return Some(Self {
                style: PathStyle::Posix,
                prefix: String::new(),
                components: rest
                    .split('/')
                    .filter(|segment| !segment.is_empty() && *segment != ".")
                    .map(str::to_owned)
                    .collect(),
            });
        }
        None
    }

    #[must_use]
    pub fn style(&self) -> PathStyle {
        self.style
    }

    /// The names below `root`, if this path lies at or below it.
    #[must_use]
    pub fn below(&self, root: &Self) -> Option<&[String]> {
        if self.style != root.style
            || self.prefix != root.prefix
            || self.components.len() < root.components.len()
        {
            return None;
        }
        let same = |left: &String, right: &String| match self.style {
            PathStyle::Windows => left.to_lowercase() == right.to_lowercase(),
            PathStyle::Posix => left == right,
        };
        self.components
            .iter()
            .zip(&root.components)
            .all(|(left, right)| same(left, right))
            .then(|| &self.components[root.components.len()..])
    }

    /// How many names deep the path is; the longer of two matching roots wins.
    #[must_use]
    pub fn depth(&self) -> usize {
        self.components.len()
    }
}

/// Whether `value` names an absolute path on this system.
#[must_use]
pub fn is_native_absolute(value: &str) -> bool {
    Path::new(value).is_absolute()
}

/// One storage root moved: its path as the backup has it, and where it lies here.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RootMapping {
    pub root_id: String,
    pub from: ForeignPath,
    pub to: PathBuf,
}

/// Why a mapping itself is refused.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MappingProblem {
    /// The backup's path for the root is no absolute path of either system.
    SourceNotAbsolute,
    /// The new place is not absolute on this system.
    TargetNotAbsolute,
    /// The new place contains `..` or `.`: it names its root through another folder.
    TargetNotPlain,
}

impl MappingProblem {
    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            Self::SourceNotAbsolute => "backup.restore_mapping_source_invalid",
            Self::TargetNotAbsolute => "backup.restore_mapping_not_absolute",
            Self::TargetNotPlain => "backup.restore_mapping_not_plain",
        }
    }
}

impl RootMapping {
    /// A mapping of the root stored as `from` to `to`.
    ///
    /// # Errors
    ///
    /// When `from` is no absolute path, or `to` is not a plain absolute path here.
    pub fn new(root_id: String, from: &str, to: &str) -> Result<Self, MappingProblem> {
        let from = ForeignPath::parse(from).ok_or(MappingProblem::SourceNotAbsolute)?;
        let to = PathBuf::from(to.trim());
        if !to.is_absolute() {
            return Err(MappingProblem::TargetNotAbsolute);
        }
        if to
            .components()
            .any(|component| matches!(component, Component::ParentDir | Component::CurDir))
        {
            return Err(MappingProblem::TargetNotPlain);
        }
        Ok(Self { root_id, from, to })
    }
}

/// What a remap made of one stored path.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Remapped {
    /// Below a moved root: its new place, inside the root's new place.
    Moved(PathBuf),
    /// Below no moved root, or not absolute at all: it stays as stored.
    Unmapped,
    /// Below a moved root, but its relative part is not plain names: refused.
    Escapes,
}

/// Moves `value` by the deepest mapping it lies below.
#[must_use]
pub fn remap(value: &str, mappings: &[RootMapping]) -> Remapped {
    let Some(path) = ForeignPath::parse(value) else {
        return Remapped::Unmapped;
    };
    let Some((mapping, names)) = mappings
        .iter()
        .filter_map(|mapping| path.below(&mapping.from).map(|names| (mapping, names)))
        .max_by_key(|(mapping, _)| mapping.from.depth())
    else {
        return Remapped::Unmapped;
    };
    let mut moved = mapping.to.clone();
    for name in names {
        // One plain name on this system too, or the remap would change the path's shape.
        let mut parts = Path::new(name).components();
        match (parts.next(), parts.next()) {
            (Some(Component::Normal(segment)), None) if segment == name.as_str() => {
                moved.push(segment);
            }
            _ => return Remapped::Escapes,
        }
    }
    if !moved.starts_with(&mapping.to) {
        return Remapped::Escapes;
    }
    Remapped::Moved(moved)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mapping(from: &str, to: &str) -> RootMapping {
        RootMapping::new("r".to_owned(), from, to).expect("mapping")
    }

    /// The target this test system can name as absolute.
    fn here(path: &str) -> String {
        if cfg!(windows) {
            format!(r"C:{}", path.replace('/', "\\"))
        } else {
            path.to_owned()
        }
    }

    #[test]
    fn windows_drive_share_and_posix_paths_are_read_by_their_own_shape() {
        let drive = ForeignPath::parse(r"D:\Downloads\Film").expect("drive");
        assert_eq!(drive.style(), PathStyle::Windows);
        assert_eq!(drive.depth(), 2);
        // The other separator and the other case name the same place.
        let same = ForeignPath::parse("d:/downloads/film").expect("drive");
        assert_eq!(same.below(&drive), Some(&[][..]));
        let share = ForeignPath::parse(r"\\NAS\Media\Film").expect("share");
        assert_eq!(share.depth(), 1);
        assert_eq!(
            ForeignPath::parse(r"\\?\UNC\nas\media\Film").map(|path| path.prefix),
            Some(share.prefix.clone())
        );
        assert_eq!(
            ForeignPath::parse(r"\\?\D:\Downloads").map(|path| path.prefix),
            Some("d:".to_owned())
        );
        let posix = ForeignPath::parse("/srv/downloads/Film").expect("posix");
        assert_eq!(posix.style(), PathStyle::Posix);
        for relative in ["downloads", "D:relative", "", r"\\server", "./x"] {
            assert_eq!(ForeignPath::parse(relative), None, "{relative}");
        }
    }

    #[test]
    fn windows_names_compare_without_case_and_posix_names_with_it() {
        let root = ForeignPath::parse(r"D:\Downloads").expect("root");
        let below = ForeignPath::parse(r"d:\DOWNLOADS\Film\a.mkv").expect("below");
        assert_eq!(
            below.below(&root),
            Some(&["Film".to_owned(), "a.mkv".to_owned()][..])
        );
        let posix_root = ForeignPath::parse("/srv/Downloads").expect("root");
        assert!(
            ForeignPath::parse("/srv/downloads/x")
                .expect("path")
                .below(&posix_root)
                .is_none()
        );
        // A sibling that shares the root's first letters is not below it.
        assert!(
            ForeignPath::parse(r"D:\Downloads2\x")
                .expect("path")
                .below(&root)
                .is_none()
        );
    }

    #[test]
    fn a_windows_root_moves_onto_this_system_with_its_relative_part() {
        let mappings = [mapping(r"D:\Downloads", &here("/srv/downloads"))];
        assert_eq!(
            remap(r"D:\Downloads\Movies\Film (2020)", &mappings),
            Remapped::Moved(
                PathBuf::from(here("/srv/downloads"))
                    .join("Movies")
                    .join("Film (2020)")
            )
        );
        assert_eq!(
            remap(r"D:\Downloads", &mappings),
            Remapped::Moved(PathBuf::from(here("/srv/downloads")))
        );
        let shares = [mapping(r"\\nas\media", &here("/mnt/media"))];
        assert_eq!(
            remap(r"\\NAS\Media\Series\S01", &shares),
            Remapped::Moved(PathBuf::from(here("/mnt/media")).join("Series").join("S01"))
        );
    }

    #[test]
    fn a_linux_root_moves_onto_a_windows_style_target_name_by_name() {
        let mappings = [mapping("/home/user/Downloads", &here("/data/dl"))];
        assert_eq!(
            remap("/home/user/Downloads/a/b.bin", &mappings),
            Remapped::Moved(PathBuf::from(here("/data/dl")).join("a").join("b.bin"))
        );
    }

    #[test]
    fn a_stored_parent_step_cannot_leave_the_moved_root() {
        let mappings = [mapping(r"D:\Downloads", &here("/srv/downloads"))];
        assert_eq!(
            remap(r"D:\Downloads\..\Windows\System32", &mappings),
            Remapped::Escapes
        );
        assert_eq!(
            remap("D:/Downloads/Film/../../etc", &mappings),
            Remapped::Escapes
        );
    }

    /// A POSIX name may hold a backslash; on POSIX it stays one name.
    #[cfg(not(windows))]
    #[test]
    fn a_posix_name_with_a_backslash_stays_one_name_here() {
        let mappings = [mapping("/srv/old", "/srv/new")];
        assert_eq!(
            remap(r"/srv/old/a\b", &mappings),
            Remapped::Moved(PathBuf::from("/srv/new").join(r"a\b"))
        );
    }

    /// On Windows the same name would become two folders, or a drive: refused, not reshaped.
    #[cfg(windows)]
    #[test]
    fn a_name_that_is_a_separator_or_a_drive_here_is_refused() {
        let mappings = [mapping("/srv/old", r"C:\new")];
        assert_eq!(remap(r"/srv/old/a\b", &mappings), Remapped::Escapes);
        assert_eq!(remap("/srv/old/C:", &mappings), Remapped::Escapes);
    }

    #[test]
    fn the_deepest_matching_root_wins_and_other_paths_stay_as_they_are() {
        let mappings = [
            mapping(r"D:\Downloads", &here("/srv/downloads")),
            mapping(r"D:\Downloads\Series", &here("/mnt/series")),
        ];
        assert_eq!(
            remap(r"D:\Downloads\Series\S01", &mappings),
            Remapped::Moved(PathBuf::from(here("/mnt/series")).join("S01"))
        );
        assert_eq!(remap(r"E:\Watch", &mappings), Remapped::Unmapped);
        assert_eq!(remap("relative/folder", &mappings), Remapped::Unmapped);
    }

    #[test]
    fn a_mapping_needs_an_absolute_source_and_a_plain_absolute_target() {
        assert_eq!(
            RootMapping::new("r".to_owned(), "downloads", &here("/srv")),
            Err(MappingProblem::SourceNotAbsolute)
        );
        assert_eq!(
            RootMapping::new("r".to_owned(), r"D:\Downloads", "srv/downloads"),
            Err(MappingProblem::TargetNotAbsolute)
        );
        assert_eq!(
            RootMapping::new("r".to_owned(), r"D:\Downloads", &here("/srv/../etc")),
            Err(MappingProblem::TargetNotPlain)
        );
        let codes = [
            MappingProblem::SourceNotAbsolute.code(),
            MappingProblem::TargetNotAbsolute.code(),
            MappingProblem::TargetNotPlain.code(),
        ];
        assert!(codes.iter().all(|code| code.starts_with("backup.restore_")));
    }
}
