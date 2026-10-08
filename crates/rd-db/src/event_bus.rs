//! The event bus: the live broadcast channel and the bounded replay buffer beside it.
//!
//! Every event goes through [`EventBus::send`], which writes the buffer and the channel under
//! one lock. That is what makes a resume exact: a subscriber that takes the buffer's tail and
//! a fresh receiver in the same critical section gets every event after its resume point once
//! -- none twice, none lost between buffer and channel. A recorder subscribed to the channel
//! could not promise that: it could itself fall behind, and an event it missed would be missing
//! from the buffer without the buffer knowing (RD-110-23).
//!
//! The buffer lives in this process and nowhere else. After a restart every id is unknown and
//! a resume is answered with [`Replay::Expired`]; a persistent event store is RD-110-03, not
//! this.

use std::{
    collections::VecDeque,
    sync::{Arc, Mutex, MutexGuard, PoisonError, Weak},
};

use rd_core::{EventEnvelope, EventId};
use tokio::sync::broadcast;

/// How many events the buffer holds at most.
///
/// The busiest event is `download.progress`, throttled by the writer to one per download every
/// 750 ms: eight parallel downloads are about eleven events a second, and 4096 then cover a
/// good six minutes -- more than the capture agent's largest reconnect interval and many times
/// the `retry:` the service sends. With nothing running, the buffer spans practically the life
/// of the process.
pub const EVENT_BUFFER_EVENTS: usize = 4096;

/// How many bytes of payload the buffer holds at most, measured as the serialised length.
///
/// The guard for the exception: `captcha.changed` carries the whole waiting list on the bus,
/// image `data:` URIs included, and 4096 of those would be well beyond a few megabytes.
pub const EVENT_BUFFER_BYTES: usize = 8 * 1024 * 1024;

/// Capacity of the live channel per subscriber before it is reported as lagged.
const LIVE_CAPACITY: usize = 512;

/// What a resume hands back beside the live receiver.
#[derive(Debug)]
pub enum Replay {
    /// Every buffered event after the resume point, oldest first; empty when nothing was
    /// missed.
    Events(Vec<EventEnvelope>),
    /// The resume point is not in the buffer: it fell out, or it was issued before a restart.
    Expired,
}

/// The live channel plus the bounded buffer, shared by the writer and every subscriber.
#[derive(Clone)]
pub struct EventBus {
    inner: Arc<Mutex<Inner>>,
}

struct Inner {
    live: broadcast::Sender<EventEnvelope>,
    buffer: VecDeque<Buffered>,
    bytes: usize,
    max_events: usize,
    max_bytes: usize,
    /// The sequence number the next event gets: how many were ever sent.
    next_seq: u64,
}

struct Buffered {
    event: EventEnvelope,
    bytes: usize,
    seq: u64,
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}

impl EventBus {
    /// A bus with the bounds every service instance runs with.
    #[must_use]
    pub fn new() -> Self {
        Self::bounded(EVENT_BUFFER_EVENTS, EVENT_BUFFER_BYTES)
    }

    /// A bus whose buffer keeps at most `events` events and `bytes` bytes of payload.
    #[must_use]
    pub fn bounded(events: usize, bytes: usize) -> Self {
        let (live, _) = broadcast::channel(LIVE_CAPACITY);
        Self {
            inner: Arc::new(Mutex::new(Inner {
                live,
                buffer: VecDeque::new(),
                bytes: 0,
                max_events: events,
                max_bytes: bytes,
                next_seq: 0,
            })),
        }
    }

    /// Records the event and hands it to every live subscriber; says how many received it.
    pub fn send(&self, event: EventEnvelope) -> usize {
        let mut inner = self.lock();
        inner.record(event.clone());
        inner.live.send(event).unwrap_or(0)
    }

    /// A receiver for everything sent from now on.
    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<EventEnvelope> {
        self.lock().live.subscribe()
    }

    /// Everything buffered after `after`, and a receiver for everything sent from now on --
    /// taken together, so nothing falls between the two.
    #[must_use]
    pub fn resume(&self, after: EventId) -> (Replay, broadcast::Receiver<EventEnvelope>) {
        let inner = self.lock();
        let live = inner.live.subscribe();
        let replay = match inner
            .buffer
            .iter()
            .rposition(|buffered| buffered.event.id == after)
        {
            Some(position) => Replay::Events(
                inner
                    .buffer
                    .iter()
                    .skip(position + 1)
                    .map(|buffered| buffered.event.clone())
                    .collect(),
            ),
            None => Replay::Expired,
        };
        (replay, live)
    }

    /// A subscriber that takes what it fell behind on from the buffer (CORE-01).
    #[must_use]
    pub fn follow(&self) -> Follower {
        let inner = self.lock();
        Follower {
            bus: Arc::downgrade(&self.inner),
            live: inner.live.subscribe(),
            next: inner.next_seq,
            replayed: VecDeque::new(),
        }
    }

    /// How many events the buffer currently holds.
    #[must_use]
    pub fn buffered(&self) -> usize {
        self.lock().buffer.len()
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        // A panic while holding the lock leaves the buffer consistent: every write is a push
        // or a pop, and neither is interrupted by anything that can panic.
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Inner {
    /// Everything buffered from sequence number `from` on and a receiver for everything sent
    /// from now on, taken under the one lock.
    fn since(&self, from: u64) -> CatchUp {
        let oldest = self
            .buffer
            .front()
            .map_or(self.next_seq, |buffered| buffered.seq);
        CatchUp {
            events: self
                .buffer
                .iter()
                .filter(|buffered| buffered.seq >= from)
                .map(|buffered| buffered.event.clone())
                .collect(),
            live: self.live.subscribe(),
            next: self.next_seq,
            lost: oldest.saturating_sub(from),
        }
    }

    fn record(&mut self, event: EventEnvelope) {
        let bytes = serde_json::to_vec(&event.payload).map_or(0, |raw| raw.len());
        self.bytes = self.bytes.saturating_add(bytes);
        let seq = self.next_seq;
        self.next_seq += 1;
        self.buffer.push_back(Buffered { event, bytes, seq });
        // The newest event always stays: a single payload over the byte bound would otherwise
        // empty the buffer, and one oversized entry is a bounded excess while an empty buffer
        // is a refused resume.
        while self.buffer.len() > 1
            && (self.buffer.len() > self.max_events || self.bytes > self.max_bytes)
        {
            if let Some(evicted) = self.buffer.pop_front() {
                self.bytes = self.bytes.saturating_sub(evicted.bytes);
            }
        }
    }
}

/// A subscriber that does not lose events to a burst (CORE-01).
///
/// The live channel holds [`LIVE_CAPACITY`] events per subscriber; one that falls further
/// behind is told `Lagged` and the channel skips ahead. A follower then takes the skipped
/// events from the buffer, with a fresh receiver from the same step, so it sees every event
/// once and in order -- the resume of RD-110-23, counted by sequence number so it also works
/// before the follower has received anything. Only what fell out of the buffer as well is
/// lost, and that is reported as `Lagged` with the number lost.
pub struct Follower {
    /// Weak, so the channel still closes when the bus goes away.
    bus: Weak<Mutex<Inner>>,
    live: broadcast::Receiver<EventEnvelope>,
    /// The sequence number of the next event the live receiver hands over.
    next: u64,
    replayed: VecDeque<EventEnvelope>,
}

impl Follower {
    /// The next event; `Lagged(n)` only when `n` events fell out of the buffer before they
    /// could be replayed, `Closed` when the bus is gone.
    pub async fn recv(&mut self) -> Result<EventEnvelope, broadcast::error::RecvError> {
        loop {
            if let Some(event) = self.replayed.pop_front() {
                return Ok(event);
            }
            match self.live.recv().await {
                Ok(event) => {
                    self.next += 1;
                    return Ok(event);
                }
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    let Some(bus) = self.bus.upgrade() else {
                        return Err(broadcast::error::RecvError::Closed);
                    };
                    let catch_up = bus
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .since(self.next);
                    self.replayed = catch_up.events.into();
                    self.live = catch_up.live;
                    self.next = catch_up.next;
                    if catch_up.lost > 0 {
                        return Err(broadcast::error::RecvError::Lagged(catch_up.lost));
                    }
                }
                Err(closed) => return Err(closed),
            }
        }
    }
}

/// What a follower that fell behind takes from the buffer, and where it goes on from.
struct CatchUp {
    /// Every buffered event from the follower's next sequence number on, oldest first.
    events: Vec<EventEnvelope>,
    /// A receiver for everything sent after them.
    live: broadcast::Receiver<EventEnvelope>,
    /// The sequence number of the first event on `live`.
    next: u64,
    /// How many events the follower missed fell out of the buffer as well.
    lost: u64,
}

#[cfg(test)]
mod tests {
    use super::{EventBus, LIVE_CAPACITY, Replay};
    use rd_core::{EventEnvelope, EventId, EventKind};
    use tokio::sync::broadcast::error::{RecvError, TryRecvError};

    fn event(mark: &str) -> EventEnvelope {
        EventEnvelope::new(
            EventKind::CollectorChanged,
            serde_json::json!({ "mark": mark }),
        )
    }

    fn marks(replay: Replay) -> Vec<String> {
        match replay {
            Replay::Events(events) => events
                .into_iter()
                .map(|event| event.payload["mark"].as_str().unwrap_or("").to_owned())
                .collect(),
            Replay::Expired => panic!("the resume point should still be in the buffer"),
        }
    }

    #[test]
    fn a_resume_hands_back_exactly_the_events_after_the_id() {
        let bus = EventBus::new();
        let first = event("a");
        let after = first.id;
        bus.send(first);
        bus.send(event("b"));
        bus.send(event("c"));

        let (replay, mut live) = bus.resume(after);
        assert_eq!(marks(replay), ["b", "c"]);

        // What comes next is on the live receiver and only there.
        bus.send(event("d"));
        let next = live.try_recv().expect("the event sent after the resume");
        assert_eq!(next.payload["mark"], "d");
        assert!(live.try_recv().is_err(), "nothing else was sent");
    }

    #[test]
    fn nothing_missed_is_an_empty_replay_not_an_expiry() {
        let bus = EventBus::new();
        let latest = event("a");
        let after = latest.id;
        bus.send(latest);

        let (replay, _live) = bus.resume(after);
        assert!(marks(replay).is_empty());
    }

    #[test]
    fn an_id_that_fell_out_of_the_buffer_is_expired() {
        let bus = EventBus::bounded(2, usize::MAX);
        let oldest = event("a");
        let fell_out = oldest.id;
        let kept = event("b");
        let still_held = kept.id;
        bus.send(oldest);
        bus.send(kept);
        bus.send(event("c"));

        assert!(matches!(bus.resume(fell_out).0, Replay::Expired));
        assert_eq!(marks(bus.resume(still_held).0), ["c"]);
        assert_eq!(bus.buffered(), 2);
    }

    #[test]
    fn an_id_the_bus_never_saw_is_expired() {
        let bus = EventBus::new();
        bus.send(event("a"));
        assert!(matches!(bus.resume(EventId::new()).0, Replay::Expired));
    }

    /// The byte bound evicts the oldest first and never the newest: a payload larger than the
    /// whole bound is kept alone rather than leaving nothing to resume from.
    #[test]
    fn the_byte_bound_evicts_the_oldest_first_and_keeps_the_newest() {
        let small = serde_json::to_vec(&event("a").payload)
            .expect("serialise")
            .len();
        let bus = EventBus::bounded(usize::MAX, small * 2);
        let first = event("a");
        let after = first.id;
        bus.send(first);
        bus.send(event("b"));
        assert_eq!(bus.buffered(), 2);
        assert_eq!(marks(bus.resume(after).0), ["b"]);

        bus.send(EventEnvelope::new(
            EventKind::CaptchaChanged,
            serde_json::json!({ "mark": "c", "image": "x".repeat(small * 4) }),
        ));
        assert_eq!(bus.buffered(), 1, "one oversized payload stands alone");
        assert!(matches!(bus.resume(after).0, Replay::Expired));
    }

    /// Buffer and channel move together: an event sent after the resume is in the receiver
    /// and not in the replay, and one sent before is in the replay and not in the receiver.
    #[test]
    fn a_send_lands_on_exactly_one_side_of_a_resume() {
        let bus = EventBus::new();
        let first = event("a");
        let after = first.id;
        bus.send(first);
        bus.send(event("b"));

        let (replay, mut live) = bus.resume(after);
        bus.send(event("c"));

        assert_eq!(marks(replay), ["b"]);
        assert_eq!(
            live.try_recv().expect("the live event").payload["mark"],
            "c"
        );
        assert!(live.try_recv().is_err());
    }

    /// A burst larger than the live channel: a plain subscriber is told it lagged and loses
    /// the oldest events, a follower takes them from the buffer and misses none (CORE-01).
    #[tokio::test]
    async fn a_follower_behind_a_full_channel_misses_nothing() {
        let bus = EventBus::new();
        let mut plain = bus.subscribe();
        let mut follower = bus.follow();
        let sent = LIVE_CAPACITY + 100;
        for n in 0..sent {
            bus.send(event(&n.to_string()));
        }

        assert!(matches!(plain.try_recv(), Err(TryRecvError::Lagged(100))));
        for n in 0..sent {
            let received = follower.recv().await.expect("every event of the burst");
            assert_eq!(received.payload["mark"], n.to_string());
        }
        bus.send(event("after"));
        let next = follower.recv().await.expect("the event after the burst");
        assert_eq!(next.payload["mark"], "after");
    }

    /// What fell out of the buffer as well is reported with its number, and the rest of the
    /// buffer still arrives in order.
    #[tokio::test]
    async fn a_follower_reports_only_what_fell_out_of_the_buffer() {
        let kept = 100;
        let bus = EventBus::bounded(kept, usize::MAX);
        let mut follower = bus.follow();
        let sent = LIVE_CAPACITY + 50;
        for n in 0..sent {
            bus.send(event(&n.to_string()));
        }

        let lost = (sent - kept) as u64;
        assert!(matches!(follower.recv().await, Err(RecvError::Lagged(n)) if n == lost));
        for n in sent - kept..sent {
            let received = follower.recv().await.expect("the buffered rest");
            assert_eq!(received.payload["mark"], n.to_string());
        }
    }
}
