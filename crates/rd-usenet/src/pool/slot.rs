//! Places on a server's lines: choosing, opening and retiring the connections a request runs
//! on, and the guards that give a place or a pending connection back.

use std::sync::{Arc, atomic::Ordering};

use anyhow::Result;

use super::{ServerPool, line::Line};
use crate::NntpClient;

impl ServerPool {
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
    pub(super) async fn slot(&self, alone: bool) -> Result<Slot<'_>> {
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
    pub(super) async fn retire(&self, line: &Arc<Line>) {
        line.broken.store(true, Ordering::Release);
        self.lines
            .lock()
            .await
            .retain(|open| !Arc::ptr_eq(open, line));
        self.changed.notify_waiters();
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
pub(super) struct Slot<'a> {
    pool: &'a ServerPool,
    pub(super) line: Arc<Line>,
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
