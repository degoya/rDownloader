//! Reading a text file whose path is configuration, without trusting it to be small.
//!
//! A password list or a domain blocklist is named in the settings and read wherever it is. A
//! path that names a device or a pipe would otherwise hold the reader for good or fill the
//! memory (audit 2026-10-05, S9).

use std::{
    io::{self, Read as _},
    path::Path,
};

/// The text of the regular file at `path`, at most `max_bytes` of it. Past the cap the partial
/// last line is dropped, so a line-based list loses whole lines only. Anything but a regular
/// file is refused before it is opened: opening a FIFO waits for a writer.
///
/// # Errors
///
/// `InvalidInput` for anything but a regular file, `InvalidData` for text that is not UTF-8,
/// and whatever reading the file fails with.
pub fn read_text_capped(path: &Path, max_bytes: usize) -> io::Result<String> {
    if !std::fs::metadata(path)?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not a regular file",
        ));
    }
    let mut bytes = Vec::new();
    std::fs::File::open(path)?
        .take(u64::try_from(max_bytes).unwrap_or(u64::MAX))
        .read_to_end(&mut bytes)?;
    if bytes.len() >= max_bytes {
        let complete = bytes
            .iter()
            .rposition(|byte| *byte == b'\n')
            .map_or(0, |end| end + 1);
        bytes.truncate(complete);
    }
    String::from_utf8(bytes).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

#[cfg(test)]
mod tests {
    use super::read_text_capped;

    #[test]
    fn a_long_file_is_cut_after_its_last_complete_line() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("list.txt");
        std::fs::write(&path, "one\ntwo\nthree\n").expect("write");
        assert_eq!(
            read_text_capped(&path, 64).expect("read"),
            "one\ntwo\nthree\n"
        );
        assert_eq!(read_text_capped(&path, 10).expect("read"), "one\ntwo\n");
        assert_eq!(read_text_capped(&path, 2).expect("read"), "");
    }

    #[test]
    fn a_directory_is_not_read() {
        let temp = tempfile::tempdir().expect("tempdir");
        let error = read_text_capped(temp.path(), 64).expect_err("a directory");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
    }

    /// A device without an end is refused at once instead of filling the memory.
    #[cfg(unix)]
    #[test]
    fn a_device_is_not_read() {
        let error = read_text_capped(std::path::Path::new("/dev/zero"), 64).expect_err("a device");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
    }
}
