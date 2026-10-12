//! What the pool's connections are doing right now, per server and per running file
//! (RD-1270-21).
//!
//! Files share the pool (RD-108-26): a connection carries up to two requests at a time, and
//! the two may belong to different files. A server's figure is plain - the lines carrying at
//! least one request. A file's figure counts the lines carrying at least one of its requests,
//! and a line two files share is split between them when the queue asks, so the files of a
//! reading never add up to more connections than the servers have busy.
//!
//! The counters move where the pool already counts: a place on a line taken or given back.
//! Nothing here waits, and nothing is written anywhere.

use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex as StdMutex, MutexGuard, PoisonError,
        atomic::{AtomicUsize, Ordering},
    },
};

use rd_core::{DownloadId, UsenetServerId};

/// The registry the runner reads its connections from.
#[derive(Clone, Default)]
pub(crate) struct UsenetConnections {
    inner: Arc<Registry>,
}

#[derive(Default)]
struct Registry {
    /// The servers of the pool in force, in its order.
    servers: StdMutex<Vec<(UsenetServerId, Arc<ServerGauge>)>>,
    files: StdMutex<HashMap<DownloadId, Arc<FileGauge>>>,
}

/// One server's lines that carry a request, and how many the pool may open.
#[derive(Default)]
pub(crate) struct ServerGauge {
    busy: AtomicUsize,
    limit: AtomicUsize,
}

impl ServerGauge {
    /// The pool's limit for this server, set whenever a pool is built for it.
    pub(crate) fn set_limit(&self, limit: usize) {
        self.limit.store(limit, Ordering::Release);
    }

    /// A line went from idle to carrying a request.
    pub(crate) fn line_busy(&self) {
        self.busy.fetch_add(1, Ordering::AcqRel);
    }

    /// A line gave back its last request.
    pub(crate) fn line_idle(&self) {
        let _ = self
            .busy
            .try_update(Ordering::AcqRel, Ordering::Acquire, |busy| {
                busy.checked_sub(1)
            });
    }
}

/// One running file's lines: those carrying at least one of its requests.
#[derive(Default)]
pub(crate) struct FileGauge {
    lines: AtomicUsize,
}

impl FileGauge {
    #[cfg(test)]
    pub(crate) fn lines(&self) -> usize {
        self.lines.load(Ordering::Acquire)
    }
}

/// The files with a request on one line, and how many requests each has there.
#[derive(Default)]
pub(crate) struct LineFiles(StdMutex<Vec<(Arc<FileGauge>, usize)>>);

impl LineFiles {
    fn lock(&self) -> MutexGuard<'_, Vec<(Arc<FileGauge>, usize)>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// One more of `file`'s requests on this line; the first one counts the line for it.
    pub(crate) fn enter(&self, file: &Arc<FileGauge>) {
        let mut files = self.lock();
        match files.iter_mut().find(|(held, _)| Arc::ptr_eq(held, file)) {
            Some((_, requests)) => *requests += 1,
            None => {
                file.lines.fetch_add(1, Ordering::AcqRel);
                files.push((Arc::clone(file), 1));
            }
        }
    }

    /// One of `file`'s requests left this line; the last one takes the line off its count.
    pub(crate) fn leave(&self, file: &Arc<FileGauge>) {
        let mut files = self.lock();
        let Some(index) = files.iter().position(|(held, _)| Arc::ptr_eq(held, file)) else {
            return;
        };
        files[index].1 -= 1;
        if files[index].1 == 0 {
            files.swap_remove(index);
            file.lines.fetch_sub(1, Ordering::AcqRel);
        }
    }
}

/// A running file's place in the registry; taken off it when dropped, however the file ends.
pub(crate) struct TrackedFile {
    registry: Arc<Registry>,
    id: DownloadId,
    gauge: Arc<FileGauge>,
}

impl TrackedFile {
    pub(crate) fn gauge(&self) -> &Arc<FileGauge> {
        &self.gauge
    }
}

impl Drop for TrackedFile {
    fn drop(&mut self) {
        let mut files = lock(&self.registry.files);
        // Only its own entry: a second attempt of the same file may have replaced it.
        if files
            .get(&self.id)
            .is_some_and(|gauge| Arc::ptr_eq(gauge, &self.gauge))
        {
            files.remove(&self.id);
        }
    }
}

impl UsenetConnections {
    /// The gauges of the pool about to be built, in its order. A server keeps its gauge
    /// across pools, so lines of a pool being replaced still count where the new ones do;
    /// a server no longer in the pool leaves the reading.
    pub(crate) fn servers(&self, ids: &[UsenetServerId]) -> Vec<Arc<ServerGauge>> {
        let mut servers = lock(&self.inner.servers);
        let gauges: Vec<Arc<ServerGauge>> = ids
            .iter()
            .map(|id| {
                servers
                    .iter()
                    .find(|(known, _)| known == id)
                    .map_or_else(Arc::default, |(_, gauge)| Arc::clone(gauge))
            })
            .collect();
        *servers = ids.iter().copied().zip(gauges.iter().cloned()).collect();
        gauges
    }

    /// Puts `id` on the books until the returned handle is dropped.
    pub(crate) fn track(&self, id: DownloadId) -> TrackedFile {
        let gauge = Arc::new(FileGauge::default());
        lock(&self.inner.files).insert(id, Arc::clone(&gauge));
        TrackedFile {
            registry: Arc::clone(&self.inner),
            id,
            gauge,
        }
    }

    /// What the connections are doing right now.
    pub(crate) fn reading(&self) -> rd_scheduler::LiveConnections {
        let servers: Vec<rd_scheduler::ServerConnections> = lock(&self.inner.servers)
            .iter()
            .map(|(id, gauge)| rd_scheduler::ServerConnections {
                server: *id,
                busy: count(gauge.busy.load(Ordering::Acquire)),
                limit: count(gauge.limit.load(Ordering::Acquire)),
            })
            .collect();
        let busy = servers.iter().map(|server| server.busy as usize).sum();
        let files: Vec<(DownloadId, usize)> = lock(&self.inner.files)
            .iter()
            .map(|(id, gauge)| (*id, gauge.lines.load(Ordering::Acquire)))
            .filter(|(_, lines)| *lines > 0)
            .collect();
        rd_scheduler::LiveConnections {
            files: shares(files, busy).into_iter().collect(),
            servers,
        }
    }
}

/// Each file's connections, scaled down when lines shared between files would otherwise be
/// counted once for each of them.
///
/// Largest remainder: every file gets the whole part of its share, and the connections left
/// over go to the largest fractions, so the files add up to exactly `busy`.
pub(crate) fn shares(files: Vec<(DownloadId, usize)>, busy: usize) -> Vec<(DownloadId, u32)> {
    let counted: usize = files.iter().map(|(_, lines)| lines).sum();
    if counted <= busy {
        return files
            .into_iter()
            .map(|(id, lines)| (id, count(lines)))
            .collect();
    }
    let mut parts: Vec<(DownloadId, usize, usize)> = files
        .into_iter()
        .map(|(id, lines)| {
            let scaled = lines.saturating_mul(busy);
            (id, scaled / counted, scaled % counted)
        })
        .collect();
    let given: usize = parts.iter().map(|(_, whole, _)| whole).sum();
    let mut order: Vec<usize> = (0..parts.len()).collect();
    order.sort_by_key(|index| std::cmp::Reverse(parts[*index].2));
    for index in order.into_iter().take(busy.saturating_sub(given)) {
        parts[index].1 += 1;
    }
    parts
        .into_iter()
        .map(|(id, whole, _)| (id, count(whole)))
        .collect()
}

fn count(value: usize) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

fn lock<T>(mutex: &StdMutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
