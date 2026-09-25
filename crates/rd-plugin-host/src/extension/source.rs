//! Host side of the `source` interface: reading the files of one package.
//!
//! The counterpart of `sink`. A post-processing step or a storage destination is given a
//! package handle, never a path, and every read is resolved against the directory that
//! handle stands for. A plugin therefore cannot name a file outside its package, because it
//! cannot name files at all — it asks for one of the entries the host listed for it.

use std::path::{Path, PathBuf};

use crate::runtime::PluginStoreState;

/// How a long-running step reports what it has done so far.
type ProgressReporter = Box<dyn Fn(u64, Option<u64>) + Send + Sync>;

/// Largest single read, so one call cannot claim the whole response budget.
pub(crate) const MAX_READ_BYTES: u32 = 1024 * 1024;

/// The package one extension invocation may read.
pub struct SourceState {
    handle: String,
    directory: PathBuf,
    /// The files the host offered. A read outside this list is refused even if the path
    /// would resolve inside the directory.
    files: Vec<String>,
    cancelled: std::sync::Arc<std::sync::atomic::AtomicBool>,
    progress: Option<ProgressReporter>,
}

impl SourceState {
    /// Builds the state for one package.
    #[must_use]
    pub fn new(handle: String, directory: PathBuf, files: Vec<String>) -> Self {
        Self {
            handle,
            directory,
            files,
            cancelled: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            progress: None,
        }
    }

    /// Where this invocation's progress goes, if the caller wanted it.
    #[must_use]
    pub(crate) fn progress(&self) -> Option<&ProgressReporter> {
        self.progress.as_ref()
    }

    /// Reports progress to the caller.
    ///
    /// `done` and `total` are the guest's own numbers, in bytes and unthrottled: a plugin may
    /// report per chunk, so a reporter that writes anything persistent does its own throttling
    /// (`rd_extract::storage_upload` does).
    #[must_use]
    pub fn with_progress(
        mut self,
        progress: impl Fn(u64, Option<u64>) + Send + Sync + 'static,
    ) -> Self {
        self.progress = Some(Box::new(progress));
        self
    }

    /// The flag a caller raises to stop a running step.
    #[must_use]
    pub fn cancellation(&self) -> std::sync::Arc<std::sync::atomic::AtomicBool> {
        self.cancelled.clone()
    }

    /// The files this invocation may read.
    #[must_use]
    pub fn files(&self) -> &[String] {
        &self.files
    }

    /// The handle the guest was given.
    #[must_use]
    pub fn handle(&self) -> &str {
        &self.handle
    }

    /// Renames one offered file, keeping the offered list in step.
    ///
    /// `to` is a bare file name: anything carrying a separator, a `..`, or an existing name is
    /// refused, so a plugin can reorganise the names inside its package and nothing else. The
    /// list is updated because a later read still addresses files by the name it was given.
    fn rename(&mut self, handle: &str, from: &str, to: &str) -> Result<(), String> {
        let source = self.resolve(handle, from)?;
        if to.is_empty()
            || to == "."
            || to == ".."
            || to.contains('/')
            || to.contains('\\')
            || std::path::Path::new(to).components().count() != 1
        {
            return Err("a new name must be a plain file name".to_owned());
        }
        if self.files.iter().any(|offered| offered == to) {
            return Err("that name is already taken in this package".to_owned());
        }
        let target = self.directory.join(to);
        if target.exists() {
            return Err("that name is already taken in this package".to_owned());
        }
        std::fs::rename(&source, &target).map_err(|error| error.to_string())?;
        for offered in &mut self.files {
            if offered == from {
                *offered = to.to_owned();
            }
        }
        Ok(())
    }

    /// Resolves one offered file to a real path, refusing anything else.
    fn resolve(&self, handle: &str, file: &str) -> Result<PathBuf, String> {
        if handle != self.handle {
            return Err("this invocation was given a different package".to_owned());
        }
        if !self.files.iter().any(|offered| offered == file) {
            return Err("that file is not part of this package".to_owned());
        }
        let candidate = self.directory.join(file);
        // Belt and braces: the name came from our own list, but a list built from a
        // directory listing is still data, and a `..` that slipped in must not resolve.
        if candidate
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
        {
            return Err("that file is not part of this package".to_owned());
        }
        Ok(candidate)
    }
}

/// Reads a bounded slice of one file.
pub(crate) fn read_at(
    state: &PluginStoreState,
    handle: &str,
    file: &str,
    offset: u64,
    length: u32,
) -> Result<Vec<u8>, String> {
    let source = state
        .source
        .as_ref()
        .ok_or_else(|| "this invocation has no package to read".to_owned())?;
    let path = source.resolve(handle, file)?;
    let length = length.min(MAX_READ_BYTES) as usize;
    read_slice(&path, offset, length).map_err(|error| error.to_string())
}

/// Size of one offered file.
pub(crate) fn size_of(state: &PluginStoreState, handle: &str, file: &str) -> Result<u64, String> {
    let source = state
        .source
        .as_ref()
        .ok_or_else(|| "this invocation has no package to read".to_owned())?;
    let path = source.resolve(handle, file)?;
    std::fs::metadata(&path)
        .map(|metadata| metadata.len())
        .map_err(|error| error.to_string())
}

fn read_slice(path: &Path, offset: u64, length: usize) -> std::io::Result<Vec<u8>> {
    use std::io::{Read, Seek, SeekFrom};

    let mut file = std::fs::File::open(path)?;
    file.seek(SeekFrom::Start(offset))?;
    let mut buffer = vec![0_u8; length];
    let mut filled = 0;
    while filled < length {
        let read = file.read(&mut buffer[filled..])?;
        if read == 0 {
            break;
        }
        filled += read;
    }
    buffer.truncate(filled);
    Ok(buffer)
}

/// The `source` interface, implemented for the invocation store.
///
/// Written once against the world the `postprocess` bindings generated; the storage world
/// maps the same interface onto it, so both types read a package the same way.
impl crate::extension::bindings::postprocess::rdownloader::plugin::source::Host
    for PluginStoreState
{
    async fn read_at(
        &mut self,
        handle: String,
        file: String,
        offset: u64,
        length: u32,
    ) -> Result<Vec<u8>, crate::component::rdownloader::plugin::types::Failure> {
        read_at(self, &handle, &file, offset, length).map_err(refused)
    }

    async fn size_of(
        &mut self,
        handle: String,
        file: String,
    ) -> Result<u64, crate::component::rdownloader::plugin::types::Failure> {
        size_of(self, &handle, &file).map_err(refused)
    }

    async fn progress(&mut self, done: u64, total: Option<u64>) {
        match self.source.as_ref().and_then(|source| source.progress()) {
            Some(report) => report(done, total),
            // Nobody asked to be told. That used to be every caller, which is how a storage
            // plugin could report its upload into nothing at all (RD-108-18); it is now the
            // post-processing steps alone, whose own pipeline reports per step rather than
            // per byte. Logged rather than dropped, so the next reader of this code finds
            // the reports instead of guessing whether they happen.
            None => tracing::debug!(done, ?total, "a plugin reported progress nobody wanted"),
        }
    }

    async fn rename(
        &mut self,
        handle: String,
        file: String,
        new_name: String,
    ) -> Result<(), crate::component::rdownloader::plugin::types::Failure> {
        self.source
            .as_mut()
            .ok_or_else(|| "this invocation has no package".to_owned())
            .and_then(|source| source.rename(&handle, &file, &new_name))
            .map_err(refused)
    }

    async fn should_stop(&mut self) -> bool {
        self.source
            .as_ref()
            .is_some_and(|source| source.cancelled.load(std::sync::atomic::Ordering::Relaxed))
    }
}

/// A refusal the guest sees as a permanent failure of its own request.
fn refused(message: String) -> crate::component::rdownloader::plugin::types::Failure {
    crate::component::rdownloader::plugin::types::Failure {
        category: crate::component::rdownloader::plugin::types::FailureKind::Permanent,
        message,
        code: Some("plugin.source_refused".to_owned()),
        params: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::SourceState;

    fn state(directory: &std::path::Path) -> SourceState {
        SourceState::new(
            "pkg-1".to_owned(),
            directory.to_path_buf(),
            vec!["one.bin".to_owned(), "sub/two.bin".to_owned()],
        )
    }

    #[test]
    fn only_the_offered_files_of_the_given_package_resolve() {
        let directory = tempfile::tempdir().expect("tempdir");
        let source = state(directory.path());
        assert!(source.resolve("pkg-1", "one.bin").is_ok());
        assert!(source.resolve("pkg-1", "sub/two.bin").is_ok());
        // A file that exists but was not offered, a package that was not given, and a
        // traversal are all the same refusal: the plugin does not name files.
        assert!(source.resolve("pkg-1", "secret.txt").is_err());
        assert!(source.resolve("pkg-2", "one.bin").is_err());
        assert!(source.resolve("pkg-1", "../../etc/passwd").is_err());
    }

    #[test]
    fn a_read_returns_only_what_is_there() {
        let directory = tempfile::tempdir().expect("tempdir");
        std::fs::write(directory.path().join("one.bin"), b"0123456789").expect("write");
        let source = state(directory.path());
        let path = source.resolve("pkg-1", "one.bin").expect("resolve");
        assert_eq!(super::read_slice(&path, 0, 4).expect("read"), b"0123");
        assert_eq!(super::read_slice(&path, 8, 100).expect("read"), b"89");
        assert!(super::read_slice(&path, 100, 4).expect("read").is_empty());
    }

    /// Renaming is the one thing on this interface that writes, so the guards matter more here
    /// than anywhere else: only offered files, only plain names, never over an existing file.
    #[test]
    fn rename_stays_inside_the_package_and_keeps_the_offered_list_in_step() {
        let directory = tempfile::tempdir().expect("tempdir");
        std::fs::write(directory.path().join("Big Buck Bunny.mkv"), b"x").expect("write");
        std::fs::write(directory.path().join("taken.mkv"), b"y").expect("write");
        let mut state = super::SourceState::new(
            "handle".to_owned(),
            directory.path().to_path_buf(),
            vec!["Big Buck Bunny.mkv".to_owned(), "taken.mkv".to_owned()],
        );

        state
            .rename("handle", "Big Buck Bunny.mkv", "Big.Buck.Bunny.mkv")
            .expect("a plain new name is allowed");
        assert!(directory.path().join("Big.Buck.Bunny.mkv").exists());
        assert!(
            state.files.iter().any(|file| file == "Big.Buck.Bunny.mkv"),
            "a later read addresses the file by its new name"
        );

        assert!(
            state.rename("handle", "nothing.mkv", "x.mkv").is_err(),
            "a file that was never offered cannot be renamed"
        );
        assert!(
            state
                .rename("other", "Big.Buck.Bunny.mkv", "x.mkv")
                .is_err(),
            "another invocation's handle is refused"
        );
        for name in ["../escape.mkv", "sub/escape.mkv", "..", "", "."] {
            assert!(
                state.rename("handle", "Big.Buck.Bunny.mkv", name).is_err(),
                "{name:?} must not be accepted as a new name"
            );
        }
        assert!(
            state
                .rename("handle", "Big.Buck.Bunny.mkv", "taken.mkv")
                .is_err(),
            "an existing file is never overwritten"
        );
        assert_eq!(
            std::fs::read(directory.path().join("taken.mkv")).expect("read"),
            b"y",
            "and it still holds its own content"
        );
    }
}
