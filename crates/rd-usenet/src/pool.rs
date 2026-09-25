//! Priority-ordered NNTP pool: reusable authenticated connections per server, each carrying
//! more than one request at a time.

use std::{
    collections::HashSet,
    sync::{
        Arc, LazyLock,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use anyhow::{Context, Result, bail};
use tokio::sync::{Mutex, Notify, Semaphore};

use crate::{DecodedArticle, NntpClient, NntpServerConfig, decode_yenc};

mod line;

use line::{Line, Named, Outcome};

/// `BODY` commands kept in flight on one connection at a time.
///
/// SABnzbd's `DEF_PIPELINING_REQUESTS` (`sabnzbd/constants.py:124`), the value it gives every
/// newly configured server. Two is what hides the round trip: while the server is still
/// sending one body the next command is already on its way, so the connection never idles
/// between articles. More buys nothing anyone has measured and costs memory per connection -
/// every request in flight is a whole article held at the receiver - and servers cap the
/// pipeline depth they accept. Not a setting on purpose: a number nobody can reason about
/// from the interface is a number nobody should be asked for.
pub const PIPELINE_DEPTH: usize = 2;

/// Attempts one server gets for one article before the pool gives up on it.
///
/// SABnzbd's default number of retries per server, and the reason its downloads survive a
/// provider that answers `400 Archive server temporarily offline.` to a quarter of all
/// commands: it asks again instead of writing a hole (RD-108-29). Everything transient
/// counts against this budget - a refusal about the server, a line that broke, a body whose
/// checksum does not match.
const ARTICLE_ATTEMPTS: usize = 3;

/// The first wait after a transient answer; it doubles per attempt up to [`MAX_BACKOFF`].
const BACKOFF: Duration = Duration::from_millis(250);
const MAX_BACKOFF: Duration = Duration::from_secs(2);

/// Passes through the request loop that cost no attempt: the confirmation read a refusal
/// needs (RD-108-27) and a line that lost its answer to an earlier request. Bounded as well,
/// so no server can keep a request spinning in here.
const MAX_PASSES: usize = 3 * ARTICLE_ATTEMPTS;

/// Servers whose answers were found out of step under pipelining, by `host:port`.
///
/// A pool lives for one file; without this, every file would probe the same server again,
/// lose a line and two articles to the probe and log the same warning. Process-wide on
/// purpose and forgotten at restart: a provider that fixes its frontend gets pipelining back
/// with the next start, not with a reinstall.
static OUT_OF_STEP: LazyLock<std::sync::Mutex<HashSet<String>>> =
    LazyLock::new(|| std::sync::Mutex::new(HashSet::new()));

/// Why an article did not arrive - and whether that says anything about the article.
///
/// The whole point of the type (RD-108-29). Only [`Self::Unavailable`] is a statement about
/// the article, and only it may cost the caller the bytes; [`Self::ServerFault`] means the
/// question was never answered and has to be asked again later.
#[derive(Debug)]
pub enum FetchError {
    /// Every server said it does not have this article.
    Unavailable(String),
    /// At least one server could not answer: a transient status, a connection that broke, a
    /// body that did not decode - each of them already retried.
    ServerFault(String),
}

impl FetchError {
    /// The detail worth logging or putting into a failure message.
    #[must_use]
    pub fn detail(&self) -> &str {
        match self {
            Self::Unavailable(detail) | Self::ServerFault(detail) => detail,
        }
    }
}

impl std::fmt::Display for FetchError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unavailable(detail) => write!(formatter, "{detail}"),
            Self::ServerFault(detail) => write!(formatter, "{detail}"),
        }
    }
}

impl std::error::Error for FetchError {}

/// Decoded article plus the number of prioritized servers tried.
pub struct PooledArticle {
    pub article: DecodedArticle,
    pub attempts: u32,
}

/// Priority-ordered NNTP pool with reusable authenticated connections per server.
#[derive(Clone)]
pub struct NntpPool {
    servers: Arc<Vec<ServerPool>>,
    max_parallel: usize,
}

impl NntpPool {
    /// Creates a lazy pool. Connections open on first use and never exceed server limits.
    pub fn new(configs: Vec<NntpServerConfig>) -> Result<Self> {
        Self::with_connection_cap(configs, None)
    }

    /// Like [`Self::new`], with `cap` bounding the connections to any one server below its
    /// own limit; `None` leaves every server at its limit (RD-108-25).
    pub fn with_connection_cap(configs: Vec<NntpServerConfig>, cap: Option<usize>) -> Result<Self> {
        if configs.is_empty() {
            bail!("no enabled NNTP server is configured");
        }
        let mut max_parallel = 0_usize;
        let mut servers = Vec::with_capacity(configs.len());
        for config in configs {
            if config.max_connections == 0 {
                bail!("NNTP server connection limit is zero");
            }
            let connections = cap.map_or(usize::from(config.max_connections), |cap| {
                usize::from(config.max_connections).min(cap.max(1))
            });
            max_parallel = max_parallel
                .checked_add(connections)
                .context("NNTP connection limit overflow")?;
            servers.push(ServerPool::new(config, connections));
        }
        Ok(Self {
            servers: Arc::new(servers),
            max_parallel,
        })
    }

    /// Connections the pool may hold open across all configured endpoints.
    #[must_use]
    pub fn max_parallel(&self) -> usize {
        self.max_parallel
    }

    /// Article requests worth keeping in flight at once.
    ///
    /// The highest-priority server's connections, each carrying [`PIPELINE_DEPTH`] - not the
    /// sum over every server. [`Self::fetch_decoded`] tries the servers in order and reaches a
    /// backup only for an article the primary refused, so requests beyond what the primary can
    /// carry would not go to the backups; they would queue in memory, each one a decoded
    /// article held until its turn. A primary plus three block accounts at twenty connections
    /// each would otherwise hold 160 articles in RAM for no throughput at all. Read from the
    /// depth in force: a server down to one command per connection (RD-108-27) gets a window
    /// of one per connection, so no request is marked `Downloading` only to wait for a line.
    #[must_use]
    pub fn max_parallel_requests(&self) -> usize {
        self.servers.first().map_or(0, |server| {
            server
                .connections
                .saturating_mul(server.depth.load(Ordering::Acquire))
        })
    }

    /// Number of priority/fallback endpoints attempted for an unavailable article.
    #[must_use]
    pub fn server_count(&self) -> usize {
        self.servers.len()
    }

    /// Commands one connection to the server at `index` may carry at once, right now.
    #[cfg(test)]
    pub(crate) fn pipeline_depth(&self, index: usize) -> usize {
        self.servers[index].depth.load(Ordering::Acquire)
    }

    /// Retrieves and verifies an article using servers in strict priority order.
    ///
    /// One server that could not answer outweighs every server that said no: a `430` from a
    /// backup account does not turn a primary's `400` into a fact about the article
    /// (RD-108-29).
    pub async fn fetch_decoded(&self, message_id: &str) -> Result<PooledArticle, FetchError> {
        let mut failures = Vec::new();
        let mut faulted = false;
        for server in self.servers.iter() {
            match server.article(message_id).await {
                Ok(article) => {
                    return Ok(PooledArticle {
                        article,
                        attempts: u32::try_from(failures.len().saturating_add(1))
                            .unwrap_or(u32::MAX),
                    });
                }
                Err(error) => {
                    faulted |= matches!(error, FetchError::ServerFault(_));
                    failures.push(format!("{}: {error}", server.config.host));
                }
            }
        }
        let detail = format!(
            "segment {message_id} failed on every server: {}",
            failures.join("; ")
        );
        Err(if faulted {
            FetchError::ServerFault(detail)
        } else {
            FetchError::Unavailable(detail)
        })
    }
}

struct ServerPool {
    config: NntpServerConfig,
    /// Connections this pool may open to the server.
    connections: usize,
    /// Commands a line may carry at once: [`PIPELINE_DEPTH`], or one once the server has
    /// shown that it does not keep pipelined answers in step (RD-108-27).
    depth: AtomicUsize,
    /// One permit per request the server may see at once: `connections * PIPELINE_DEPTH`.
    requests: Semaphore,
    lines: Mutex<Vec<Arc<Line>>>,
    /// Connections being opened right now; they count against the limit before they exist.
    /// Atomic rather than under `lines`, so the guard that releases one needs no lock in
    /// `Drop` - a connect that is cancelled mid-way must not leak its count.
    connecting: AtomicUsize,
    /// Signalled whenever a line is added, retired or finishes a request, or a connect ends,
    /// for a caller that found every line full while another one was still connecting.
    changed: Notify,
}

impl ServerPool {
    fn new(config: NntpServerConfig, connections: usize) -> Self {
        let depth = if out_of_step().contains(&endpoint(&config)) {
            1
        } else {
            PIPELINE_DEPTH
        };
        Self {
            requests: Semaphore::new(connections.saturating_mul(PIPELINE_DEPTH)),
            connections,
            depth: AtomicUsize::new(depth),
            config,
            lines: Mutex::new(Vec::new()),
            connecting: AtomicUsize::new(0),
            changed: Notify::new(),
        }
    }

    /// One decoded article from this server, asked for again while that is worth doing.
    ///
    /// Three kinds of answer end the loop. A body that decodes is the article. A `430` -
    /// confirmed on a line of its own when it was read beside another request, because a
    /// refusal names no message-id and cannot be verified the way a body can - is this
    /// server saying it does not have it. Everything else is transient: the server refused
    /// for its own reasons, the line broke, or the body did not decode. Those cost an
    /// attempt and a short wait, and are asked again on a fresh line, up to
    /// [`ARTICLE_ATTEMPTS`]. What they never do is end as an answer about the article.
    async fn article(&self, message_id: &str) -> Result<DecodedArticle, FetchError> {
        let _request = self
            .requests
            .acquire()
            .await
            .map_err(|_| FetchError::ServerFault("NNTP pool closed".to_owned()))?;
        // A refusal read on a line that carried another request at the time. It cannot be
        // verified the way a body can - a `430` names no message-id - so it is believed only
        // once the same question, asked on a line of its own, gets the same answer.
        let mut unconfirmed = None;
        let mut faults = 0_usize;
        // The first one, not the last: it names what actually went wrong, where a later
        // attempt often only reports that the server has stopped taking connections since.
        let mut first_fault = String::new();
        for _ in 0..MAX_PASSES {
            if faults >= ARTICLE_ATTEMPTS {
                break;
            }
            let slot = match self.slot(unconfirmed.is_some()).await {
                Ok(slot) => slot,
                // A server that will not take another connection right now is as transient
                // as one that answers `400`, and is counted the same way.
                Err(error) => {
                    self.count_fault(&mut faults, &mut first_fault, format!("{error:#}"))
                        .await;
                    continue;
                }
            };
            let fault = match slot.line.exchange(message_id).await {
                Outcome::Body { data, named } => {
                    self.note_naming(&named, message_id);
                    match decode_yenc(&data) {
                        Ok(article) => return Ok(article),
                        // A body that does not decode is a body this server sent wrong: cut
                        // short, or with a checksum that does not match. Asking it again is
                        // what SABnzbd does; writing the hole would leave the repair to PAR2.
                        Err(error) => Some(format!("{error:#}")),
                    }
                }
                Outcome::Unavailable { status, alone } => {
                    if alone || self.depth.load(Ordering::Acquire) == 1 {
                        return Err(FetchError::Unavailable(format!(
                            "NNTP server returned {status:?}"
                        )));
                    }
                    unconfirmed = Some(status);
                    continue;
                }
                Outcome::ServerFault { status } => {
                    self.retire(&slot.line).await;
                    Some(format!("NNTP server returned {status:?}"))
                }
                Outcome::Swapped { answered } => {
                    self.retire(&slot.line).await;
                    tracing::warn!(
                        host = %self.config.host,
                        requested = message_id,
                        answered,
                        "NNTP server answered out of step under pipelining"
                    );
                    self.keep_in_step("its answers were out of step");
                    continue;
                }
                Outcome::Broken(error) => {
                    self.retire(&slot.line).await;
                    Some(format!("{error:#}"))
                }
                // An earlier request took this line down before the answer arrived. It cost
                // nothing and says nothing; a fresh line answers the same question.
                Outcome::Collateral => {
                    self.retire(&slot.line).await;
                    continue;
                }
            };
            drop(slot);
            if let Some(reason) = fault {
                self.count_fault(&mut faults, &mut first_fault, reason)
                    .await;
            }
        }
        if faults > 0 {
            return Err(FetchError::ServerFault(format!(
                "{first_fault} (after {faults} attempts)"
            )));
        }
        if let Some(status) = unconfirmed {
            return Err(FetchError::Unavailable(format!(
                "NNTP server returned {status:?}"
            )));
        }
        Err(FetchError::ServerFault(format!(
            "NNTP connection to {} kept breaking under earlier requests",
            self.config.host
        )))
    }

    /// Books one transient answer against the budget and waits before the next attempt.
    async fn count_fault(&self, faults: &mut usize, first: &mut String, reason: String) {
        // The only place an absorbed refusal is visible. A provider that answers `400` to a
        // quarter of all commands used to show up as missing segments and a broken archive;
        // now it shows up here, and nowhere else, because the retry hides it from the file.
        tracing::debug!(
            host = %self.config.host,
            attempt = *faults + 1,
            reason = %reason,
            "NNTP request failed transiently; asking this server again"
        );
        if *faults == 0 {
            *first = reason;
        }
        *faults += 1;
        if *faults < ARTICLE_ATTEMPTS {
            tokio::time::sleep(backoff(*faults)).await;
        }
    }

    /// What the `222` line named, and what that says about pipelining on this server.
    fn note_naming(&self, named: &Named, message_id: &str) {
        match named {
            Named::Requested => {}
            Named::Nothing => {
                self.keep_in_step("the server names no message-id on its 222 line");
            }
            Named::Other(answered) => self.keep_in_step(&format!(
                "its 222 line named {answered} for {message_id}, an id nobody asked for"
            )),
        }
    }

    /// One command per connection from now on, for this server, for the rest of the process.
    ///
    /// The wire behaviour before RD-108-25, which the field showed this server to be fine
    /// with. Logged once per process, at the moment the pipelining is given up.
    fn keep_in_step(&self, reason: &str) {
        if self.depth.swap(1, Ordering::AcqRel) == 1 {
            return;
        }
        out_of_step().insert(endpoint(&self.config));
        tracing::warn!(
            host = %self.config.host,
            reason,
            "NNTP pipelining given up for this server; one command per connection from now on"
        );
        self.changed.notify_waiters();
    }

    /// A line with room for one more request, opened if need be.
    ///
    /// An idle line is taken first, then a new one while the limit allows, then the least
    /// loaded one: connections are what a provider meters, so every allowed connection is
    /// put to work before any of them carries a second request. With `alone`, a line is
    /// never shared: the caller waits for one of its own, and holds it reserved so nobody
    /// joins it while the request runs.
    ///
    /// The place on a line is taken under the same lock that chose it. Taken afterwards,
    /// two callers could choose the same idle line and both believe they had it alone -
    /// which is what a refusal's confirmation read exists to rule out.
    async fn slot(&self, alone: bool) -> Result<Slot<'_>> {
        loop {
            let notified = self.changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let choice = {
                let mut lines = self.lines.lock().await;
                lines.retain(|line| !line.broken.load(Ordering::Acquire));
                let connecting = self.connecting.load(Ordering::Acquire);
                let room = lines.len().saturating_add(connecting) < self.connections;
                let depth = self.depth.load(Ordering::Acquire);
                let open = |line: &&Arc<Line>| !line.reserved.load(Ordering::Acquire);
                let idle = lines
                    .iter()
                    .filter(open)
                    .find(|line| line.in_flight.load(Ordering::Acquire) == 0);
                match (idle, room) {
                    (Some(line), _) => Choice::Use(self.take(line, alone)),
                    // Reserved under the lock, so two callers cannot both see the same room.
                    (None, true) => Choice::Connect(Connecting::reserve(self)),
                    (None, false) if alone => Choice::Wait,
                    (None, false) => lines
                        .iter()
                        .filter(open)
                        .filter(|line| line.in_flight.load(Ordering::Acquire) < depth)
                        .min_by_key(|line| line.in_flight.load(Ordering::Acquire))
                        .map_or(Choice::Wait, |line| Choice::Use(self.take(line, false))),
                }
            };
            match choice {
                Choice::Use(slot) => return Ok(slot),
                Choice::Connect(reservation) => {
                    let client = NntpClient::connect(&self.config).await?;
                    let line = Arc::new(Line::new(client));
                    // Taken before the line is visible to anyone else.
                    let slot = self.take(&line, alone);
                    self.lines.lock().await.push(line);
                    drop(reservation);
                    return Ok(slot);
                }
                Choice::Wait => notified.await,
            }
        }
    }

    /// One place on `line`, counted at once; `exclusive` keeps everyone else off it.
    fn take(&self, line: &Arc<Line>, exclusive: bool) -> Slot<'_> {
        line.in_flight.fetch_add(1, Ordering::AcqRel);
        if exclusive {
            line.reserved.store(true, Ordering::Release);
        }
        Slot {
            pool: self,
            line: Arc::clone(line),
            exclusive,
        }
    }

    /// Drops a line that is no longer in step; whatever it still owes its callers is lost.
    async fn retire(&self, line: &Arc<Line>) {
        line.broken.store(true, Ordering::Release);
        self.lines
            .lock()
            .await
            .retain(|open| !Arc::ptr_eq(open, line));
        self.changed.notify_waiters();
    }
}

/// The wait before attempt `attempt + 1`, doubling and capped.
fn backoff(attempt: usize) -> Duration {
    let doubled = BACKOFF.saturating_mul(1_u32 << attempt.min(8));
    doubled.min(MAX_BACKOFF)
}

fn endpoint(config: &NntpServerConfig) -> String {
    format!("{}:{}", config.host, config.port)
}

fn out_of_step() -> std::sync::MutexGuard<'static, HashSet<String>> {
    match OUT_OF_STEP.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

enum Choice<'a> {
    Use(Slot<'a>),
    Connect(Connecting<'a>),
    Wait,
}

/// A reserved place for a connection that is still being opened.
///
/// Released on drop - after the line is in the pool, after a failed connect, and when the
/// caller's future is dropped mid-handshake, which a shutdown does. Without the guard a
/// cancelled connect would keep counting against the limit for the pool's lifetime, and at
/// one connection per server every later caller would wait forever.
struct Connecting<'a> {
    pool: &'a ServerPool,
}

impl<'a> Connecting<'a> {
    fn reserve(pool: &'a ServerPool) -> Self {
        pool.connecting.fetch_add(1, Ordering::AcqRel);
        Self { pool }
    }
}

impl Drop for Connecting<'_> {
    fn drop(&mut self) {
        self.pool.connecting.fetch_sub(1, Ordering::AcqRel);
        self.pool.changed.notify_waiters();
    }
}

/// One request's place on a line; releases it on drop, however the request ended.
struct Slot<'a> {
    pool: &'a ServerPool,
    line: Arc<Line>,
    exclusive: bool,
}

impl Drop for Slot<'_> {
    fn drop(&mut self) {
        if self.exclusive {
            self.line.reserved.store(false, Ordering::Release);
        }
        self.line.in_flight.fetch_sub(1, Ordering::AcqRel);
        self.pool.changed.notify_waiters();
    }
}
