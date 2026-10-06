//! Turning a remote directory into the reviewable [`RemoteListing`].
//!
//! `MLSD` is preferred because its output is machine-defined; `LIST` is the fallback and is
//! parsed with `suppaftp`'s POSIX and DOS line parsers. Servers disagree wildly about
//! `LIST` formatting, so a line that parses as neither is skipped rather than guessed at.

use chrono::{DateTime, Utc};
use rd_core::{ByteCount, RemoteEntry, RemoteListing};
use rd_transfer_file::{DirectoryLister, ListedEntry};
use suppaftp::list::{File, ListParser};

use crate::client::Connection;

/// Walks `root` recursively and returns everything below it, through the walk FTP shares with
/// SFTP (`rd_transfer_file::walk`): bounded on entry count and depth, breadth first.
pub(crate) async fn walk(connection: &mut Connection, root: &str) -> anyhow::Result<RemoteListing> {
    let (entries, truncated) =
        rd_transfer_file::walk(&mut FtpLister(connection), root, "ftp").await?;
    Ok(RemoteListing {
        root: root.to_owned(),
        single_file: false,
        entries,
        truncated,
        supports_resume: false,
    })
}

/// One FTP directory read: `MLSD` where the server has it, `LIST` otherwise.
struct FtpLister<'a>(&'a mut Connection);

impl DirectoryLister for FtpLister<'_> {
    async fn list(&mut self, absolute: &str) -> anyhow::Result<Option<Vec<ListedEntry>>> {
        let (lines, dialect) = match self.0.mlsd(Some(absolute)).await {
            Ok(lines) => (lines, Dialect::Mlsd),
            // Not every server implements MLSD; LIST is the universal fallback.
            Err(_) => (self.0.list(Some(absolute)).await?, Dialect::List),
        };
        Ok(Some(
            lines
                .iter()
                .filter_map(|line| parse(line, dialect))
                .map(|file| {
                    let is_dir = file.is_directory();
                    ListedEntry {
                        name: file.name().to_owned(),
                        entry: RemoteEntry {
                            path: String::new(),
                            is_dir,
                            size: (!is_dir)
                                .then(|| ByteCount::new(file.size() as u64).ok())
                                .flatten(),
                            modified: modified_at(&file),
                            etag: None,
                        },
                        descend: is_dir && !file.is_symlink(),
                    }
                })
                .collect(),
        ))
    }
}

/// Which command produced a batch of lines.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Dialect {
    /// `MLSD`, whose `facts; name` form is defined by RFC 3659.
    Mlsd,
    /// `LIST`, whose output is whatever the server's operating system prints.
    List,
}

/// Parses one listing line.
///
/// The parser is chosen by the command that produced the line rather than by trying all of
/// them: `ListParser::parse_mlsd` accepts a POSIX line and hands back the entire line as the
/// name, so a fallback chain silently produces entries named
/// `-rw-r--r-- 1 owner group 1234 Jan 10 12:00 movie.mkv`.
fn parse(line: &str, dialect: Dialect) -> Option<File> {
    let trimmed = line.trim_end_matches(['\r', '\n']);
    if trimmed.trim().is_empty() {
        return None;
    }
    match dialect {
        // Some servers answer MLSD with LIST-style output. `ListParser::parse_mlsd` would accept
        // that and hand back the whole line as the file name, so the RFC 3659 shape
        // (`fact=value;... SP pathname`) is checked before trusting it.
        Dialect::Mlsd if !has_mlsd_facts(trimmed) => ListParser::parse_posix(trimmed)
            .or_else(|_| ListParser::parse_dos(trimmed))
            .ok(),
        Dialect::Mlsd => ListParser::parse_mlsd(trimmed).ok(),
        // `ls` prefixes its output with a `total <blocks>` summary line that names no file.
        Dialect::List if trimmed.starts_with("total ") => None,
        Dialect::List => ListParser::parse_posix(trimmed)
            .or_else(|_| ListParser::parse_dos(trimmed))
            .ok(),
    }
}

/// Whether a line carries an RFC 3659 fact list before its path name.
fn has_mlsd_facts(line: &str) -> bool {
    let Some((facts, name)) = line.split_once(' ') else {
        return false;
    };
    !name.is_empty() && facts.ends_with(';') && facts.contains('=')
}

fn modified_at(file: &File) -> Option<DateTime<Utc>> {
    DateTime::<Utc>::from(file.modified()).into()
}

#[cfg(test)]
mod tests {
    use super::{Dialect, parse};

    #[test]
    fn posix_and_dos_lines_both_parse() {
        let posix = parse(
            "-rw-r--r-- 1 owner group 1234 Jan 10 12:00 movie.mkv",
            Dialect::List,
        )
        .expect("posix");
        assert_eq!(posix.name(), "movie.mkv");
        assert!(posix.is_file());
        assert_eq!(posix.size(), 1234);

        let dos = parse(
            "01-10-26  12:00PM       <DIR>          extras",
            Dialect::List,
        )
        .expect("dos");
        assert_eq!(dos.name(), "extras");
        assert!(dos.is_directory());
    }

    #[test]
    fn an_mlsd_line_parses_with_its_facts() {
        let file = parse(
            "type=file;size=42;modify=20260110120000; movie.mkv",
            Dialect::Mlsd,
        )
        .expect("mlsd");
        assert_eq!(file.name(), "movie.mkv");
        assert_eq!(file.size(), 42);
        assert!(file.is_file());
    }

    #[test]
    fn a_posix_line_answered_to_mlsd_is_still_parsed_correctly() {
        // The bug this locks in: `ListParser::parse_mlsd` accepts a POSIX line and returns the
        // whole line as the name, so a server that answers MLSD with LIST output would
        // produce an entry called
        // `-rw-r--r-- 1 owner group 1234 Jan 10 12:00 movie.mkv`.
        let line = "-rw-r--r-- 1 owner group 1234 Jan 10 12:00 movie.mkv";
        let file = parse(line, Dialect::Mlsd).expect("falls back to the posix parser");
        assert_eq!(file.name(), "movie.mkv");
        assert_eq!(file.size(), 1234);
    }

    #[test]
    fn only_a_real_fact_list_is_treated_as_mlsd() {
        assert!(super::has_mlsd_facts("type=file;size=42; movie.mkv"));
        assert!(!super::has_mlsd_facts(
            "-rw-r--r-- 1 owner group 1234 Jan 10 12:00 movie.mkv"
        ));
        // Facts but no name, and a name but no facts, are both malformed.
        assert!(!super::has_mlsd_facts("type=file;size=42; "));
        assert!(!super::has_mlsd_facts("movie.mkv"));
    }

    #[test]
    fn an_unparseable_line_is_skipped_rather_than_guessed() {
        // `ls` prints this summary before the entries; it names no file.
        assert!(parse("total 12", Dialect::List).is_none());
        assert!(parse("", Dialect::List).is_none());
        assert!(parse("   ", Dialect::Mlsd).is_none());
    }
}
