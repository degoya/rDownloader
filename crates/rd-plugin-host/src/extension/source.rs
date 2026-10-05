//! Host side of the `source` interface: reading the files of one package.
//!
//! The counterpart of `sink`. A post-processing step or a storage destination is given a
//! package handle, never a path, and every read is resolved against the directory that
//! handle stands for. A plugin therefore cannot name a file outside its package, because it
//! cannot name files at all — it asks for one of the entries the host listed for it.

use std::path::PathBuf;

use crate::runtime::PluginStoreState;

/// How a long-running step reports what it has done so far.
type ProgressReporter = Box<dyn Fn(u64, Option<u64>) + Send + Sync>;

/// Largest single read, so one call cannot claim the whole response budget.
pub(crate) const MAX_READ_BYTES: u32 = 1024 * 1024;

/// The interface `read-at` is re-linked into, with the store in hand (RD-191-06, PLUG-01).
const INTERFACE: &str = "rdownloader:plugin/source@0.10.0";

/// Fuel a read credits for every byte it hands the guest (RD-191-06, PLUG-01).
///
/// A checksum step's work grows with the file and its fuel did not: SHA-256 costs the guest
/// about 77 fuel a byte, so the 5e8 of its manifest ran out at 16 MiB and MD5's at 64 MiB.
/// Paying for the bytes as they are read leaves the manifest's budget for everything else the
/// step does, at any file size, without a change to the contract. Rounded well up from
/// SHA-256, the most expensive of the bundled steps.
pub(crate) const FUEL_PER_READ_BYTE: u64 = 128;

/// Fewest times over the package the credit reaches: a step that reads a file twice.
pub(crate) const MIN_CREDITED_PASSES: u64 = 2;

/// Most times over the package the credit reaches, however many files it holds.
pub(crate) const MAX_CREDITED_PASSES: u64 = 8;

/// How many times over the package the credit reaches (RA-HOST-05).
///
/// The ceiling that keeps a looping plugin stoppable: reading the same slice again and again
/// earns fuel only until the package has been paid for this often, and from then on the
/// manifest's own budget runs out as it always did. A fixed two left a legitimate step short:
/// every sidecar a checksum step finds — `.md5`, `.sfv`, a second `.md5` for the same release —
/// is one more pass over the files it names, so a package with three sidecars over one large
/// file ran out on the third. The host cannot tell a sidecar from a payload, so every offered
/// file counts as one possible pass, plus the pass that reads the sidecars themselves; at
/// least [`MIN_CREDITED_PASSES`], and never more than [`MAX_CREDITED_PASSES`], so a package
/// of a thousand small files does not turn one large file's credit into a loop allowance.
pub(crate) fn credited_passes(files: usize) -> u64 {
    u64::try_from(files)
        .unwrap_or(u64::MAX)
        .saturating_add(1)
        .clamp(MIN_CREDITED_PASSES, MAX_CREDITED_PASSES)
}

/// The package one extension invocation may read.
pub struct SourceState {
    handle: String,
    directory: PathBuf,
    /// The files the host offered. A read outside this list is refused even if the path
    /// would resolve inside the directory.
    files: Vec<String>,
    cancelled: std::sync::Arc<std::sync::atomic::AtomicBool>,
    progress: Option<ProgressReporter>,
    /// Paces the reads of an upload destination (RD-150-15); `None` for a post-processing
    /// step, which reads the package without sending it anywhere.
    bandwidth: Option<rd_limits::ScopedLimiter>,
    /// Fuel the reads may still credit; `None` until the first read has measured the package.
    read_credit: Option<u64>,
    /// The file the last read opened, kept for the next slice of the same file (PLUG-20): a
    /// checksum reads a file in 256 KiB slices, and opening and seeking it for every one of
    /// them was most of the work the host did.
    open: Option<(PathBuf, std::fs::File)>,
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
            bandwidth: None,
            read_credit: None,
            open: None,
        }
    }

    /// Paces every read by the upload limit.
    ///
    /// The reads are the one place an upload's bytes pass the host before they leave: a
    /// destination reads a slice and sends it, so holding the slice back until the limiter has
    /// released it keeps the upload at the limit whatever the plugin does with it afterwards.
    #[must_use]
    pub fn with_bandwidth(mut self, bandwidth: rd_limits::ScopedLimiter) -> Self {
        self.bandwidth = Some(bandwidth);
        self
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
    /// refused, so a plugin can reorganise the names inside its package and nothing else. A file
    /// in a folder of the package (`Film/film.mkv`, RD-170-16) keeps its folder. The list is
    /// updated because a later read still addresses files by the name it was given. The file
    /// system is touched off the async worker (PLUG-20).
    async fn rename(&mut self, handle: &str, from: &str, to: &str) -> Result<(), String> {
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
        let renamed = match from.rsplit_once('/') {
            Some((folder, _)) => format!("{folder}/{to}"),
            None => to.to_owned(),
        };
        if self.files.contains(&renamed) {
            return Err("that name is already taken in this package".to_owned());
        }
        let target = source.with_file_name(to);
        // The kept handle names the old path; dropped, so no later read compares against it.
        self.open = None;
        tokio::task::spawn_blocking(move || {
            if target.exists() {
                return Err("that name is already taken in this package".to_owned());
            }
            std::fs::rename(&source, &target).map_err(|error| error.to_string())
        })
        .await
        .map_err(|error| error.to_string())??;
        for offered in &mut self.files {
            if offered == from {
                offered.clone_from(&renamed);
            }
        }
        Ok(())
    }

    /// Takes the fuel a read of `bytes` earns, within what is left of the ceiling.
    fn take_read_credit(&mut self, bytes: usize) -> u64 {
        let wanted = u64::try_from(bytes)
            .unwrap_or(u64::MAX)
            .saturating_mul(FUEL_PER_READ_BYTE);
        let left = self.read_credit.get_or_insert(0);
        let credit = wanted.min(*left);
        *left -= credit;
        credit
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
///
/// The file system is touched on a blocking thread, not on the async worker the plugin runs on
/// (PLUG-20), and the file stays open for the next slice. The first read also measures the
/// package, which sets the ceiling of the fuel the reads may credit.
pub(crate) async fn read_at(
    state: &mut PluginStoreState,
    handle: &str,
    file: &str,
    offset: u64,
    length: u32,
) -> Result<Vec<u8>, String> {
    let source = state
        .source
        .as_mut()
        .ok_or_else(|| "this invocation has no package to read".to_owned())?;
    let path = source.resolve(handle, file)?;
    let length = length.min(MAX_READ_BYTES) as usize;
    let open = source
        .open
        .take()
        .filter(|(opened, _)| *opened == path)
        .map(|(_, file)| file);
    let measure = source.read_credit.is_none().then(|| {
        source
            .files
            .iter()
            .map(|offered| source.directory.join(offered))
            .collect::<Vec<_>>()
    });
    let (path, opened, bytes, measured) =
        tokio::task::spawn_blocking(move || -> std::io::Result<_> {
            let measured = measure.map(|paths| package_bytes(&paths));
            let mut opened = match open {
                Some(file) => file,
                None => std::fs::File::open(&path)?,
            };
            let bytes = read_slice(&mut opened, offset, length)?;
            Ok((path, opened, bytes, measured))
        })
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())?;
    source.open = Some((path, opened));
    if let Some(bytes) = measured {
        source.read_credit = Some(
            bytes
                .saturating_mul(FUEL_PER_READ_BYTE)
                .saturating_mul(credited_passes(source.files.len())),
        );
    }
    Ok(bytes)
}

/// Bytes of every offered file together; a file that cannot be read counts nothing.
fn package_bytes(paths: &[PathBuf]) -> u64 {
    paths
        .iter()
        .filter_map(|path| std::fs::metadata(path).ok())
        .fold(0_u64, |total, metadata| {
            total.saturating_add(metadata.len())
        })
}

/// Size of one offered file.
pub(crate) async fn size_of(
    state: &PluginStoreState,
    handle: &str,
    file: &str,
) -> Result<u64, String> {
    let source = state
        .source
        .as_ref()
        .ok_or_else(|| "this invocation has no package to read".to_owned())?;
    let path = source.resolve(handle, file)?;
    tokio::task::spawn_blocking(move || std::fs::metadata(&path).map(|metadata| metadata.len()))
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())
}

fn read_slice(file: &mut std::fs::File, offset: u64, length: usize) -> std::io::Result<Vec<u8>> {
    use std::io::{Read, Seek, SeekFrom};

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

/// Links `read-at` a second time, over the generated binding, with the store in hand.
///
/// A generated host function is handed the store's data, and fuel lives on the store itself —
/// the reason `keyderive` is hand-wired too. This one delegates the read to the generated
/// implementation below, then credits the guest [`FUEL_PER_READ_BYTE`] for every byte it was
/// handed, up to [`credited_passes`] times the package (RD-191-06, PLUG-01). Shadowing is
/// allowed for exactly this one definition.
pub(crate) fn add_metered_read_to_linker(
    linker: &mut wasmtime::component::Linker<PluginStoreState>,
) -> anyhow::Result<()> {
    linker.allow_shadowing(true);
    let defined = linker.instance(INTERFACE).and_then(|mut source| {
        source.func_wrap_async::<(String, String, u64, u32), (Result<Vec<u8>, WitFailure>,), _>(
            "read-at",
            |mut store, (handle, file, offset, length)| {
                Box::new(async move {
                    Ok((metered_read(&mut store, handle, file, offset, length).await,))
                })
            },
        )
    });
    linker.allow_shadowing(false);
    defined.map_err(anyhow::Error::from)
}

type WitFailure = crate::component::rdownloader::plugin::types::Failure;

async fn metered_read(
    store: &mut wasmtime::StoreContextMut<'_, PluginStoreState>,
    handle: String,
    file: String,
    offset: u64,
    length: u32,
) -> Result<Vec<u8>, WitFailure> {
    use crate::extension::bindings::postprocess::rdownloader::plugin::source::Host;

    let bytes =
        <PluginStoreState as Host>::read_at(store.data_mut(), handle, file, offset, length).await?;
    let credit = store
        .data_mut()
        .source
        .as_mut()
        .map_or(0, |source| source.take_read_credit(bytes.len()));
    if credit > 0 {
        let fuel = store
            .get_fuel()
            .map_err(|error| refused(error.to_string()))?;
        store
            .set_fuel(fuel.saturating_add(credit))
            .map_err(|error| refused(error.to_string()))?;
    }
    Ok(bytes)
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
        // The disk is waiting, not the plugin thinking (RA-HOST-05): a checksum over a large
        // file on a slow disk spent its execution deadline in reads and was stopped as a
        // plugin that had run too long, with nothing kept.
        let started = std::time::Instant::now();
        let read = read_at(self, &handle, &file, offset, length).await;
        self.credit_host_time(started.elapsed());
        let bytes = read.map_err(refused)?;
        if let Some(limiter) = self
            .source
            .as_ref()
            .and_then(|source| source.bandwidth.clone())
            && !bytes.is_empty()
        {
            let waited = std::time::Instant::now();
            let acquired = limiter.acquire(bytes.len()).await;
            // Waiting on the limit is not the plugin thinking, as with a socket read.
            self.credit_host_time(waited.elapsed());
            acquired.map_err(|error| refused(error.to_string()))?;
        }
        Ok(bytes)
    }

    async fn size_of(
        &mut self,
        handle: String,
        file: String,
    ) -> Result<u64, crate::component::rdownloader::plugin::types::Failure> {
        size_of(self, &handle, &file).await.map_err(refused)
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
        let source = self
            .source
            .as_mut()
            .ok_or_else(|| refused("this invocation has no package".to_owned()))?;
        source
            .rename(&handle, &file, &new_name)
            .await
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
#[path = "source_tests.rs"]
mod tests;
