//! One NNTP connection carrying up to [`PIPELINE_DEPTH`](super::PIPELINE_DEPTH) requests, and
//! the check that each answer read on it belongs to the request reading it (RD-108-27).

use std::{
    collections::VecDeque,
    sync::{
        Mutex as StdMutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

use tokio::sync::{Mutex, oneshot};

use crate::{
    NntpClient,
    nntp::{BodyError, NntpReader, NntpWriter},
};

/// One connection, shared by up to `PIPELINE_DEPTH` requests.
///
/// NNTP answers in command order, so the requests take turns: each one sends under the
/// writer lock and, in doing so, takes its place behind the request that sent before it.
/// It reads only once that one has finished reading. A request that stops before its answer
/// is fully read - an error, a timeout, a cancelled future - leaves bytes on the line that
/// belong to nobody, and the line is marked broken so nothing reads them as its own answer.
///
/// The turn order is what the client can guarantee; that the server's answers follow the
/// same order is what it cannot. So every body is checked against the command it is read
/// for. A `222` line naming another request *of this line* means the answers were swapped:
/// the line is broken on the spot, before the request behind reads anything from it. A
/// `222` line naming an id this line never asked for is a server whose ids cannot be
/// checked - a different spelling, a normalised domain - and the body is kept; the pool
/// then stops pipelining to that server rather than throwing every article away.
pub(super) struct Line {
    writer: Mutex<Writer>,
    reader: Mutex<NntpReader>,
    pub(super) in_flight: AtomicUsize,
    pub(super) broken: AtomicBool,
    /// Held by a request that must not share the line: a refusal being confirmed.
    pub(super) reserved: AtomicBool,
    /// The last few message-ids sent on this line, in send order.
    ///
    /// Not only the ones whose answers are still owed: a request that took a refusal meant
    /// for the one behind it has already left when that one reads the body naming the
    /// first request's id, and the swap is only recognisable by remembering the id.
    sent: StdMutex<VecDeque<String>>,
}

/// Ids remembered per line: the depth twice over, so a swap is still recognised after the
/// request it belonged to has left.
const REMEMBERED: usize = 2 * super::PIPELINE_DEPTH + 2;

struct Writer {
    half: NntpWriter,
    /// Resolves when the most recently sent request has finished reading its answer.
    tail: oneshot::Receiver<()>,
}

/// What the `222` line said about the body it introduced.
pub(super) enum Named {
    /// The requested id.
    Requested,
    /// No id at all.
    Nothing,
    /// An id nobody on this line asked for; the server's ids cannot be checked.
    Other(String),
}

pub(super) enum Outcome {
    /// A body for this request, with what its `222` line named.
    Body { data: Vec<u8>, named: Named },
    /// The server does not have this article; the line stays in step. `alone` says no other
    /// request was on the line when the answer was read.
    Unavailable { status: String, alone: bool },
    /// The server could not answer at all. The line goes with it: after a `400` the server
    /// closes the connection anyway (RFC 3977 §3.2.1), and the answer says nothing about
    /// whether the article exists (RD-108-29).
    ServerFault { status: String },
    /// The server answered with the body owed to another request on this line.
    Swapped { answered: String },
    /// This request broke the line.
    Broken(anyhow::Error),
    /// An earlier request broke the line and took this one's answer with it.
    Collateral,
}

impl Line {
    pub(super) fn new(client: NntpClient) -> Self {
        let (writer, reader) = client.into_halves();
        let (done, tail) = oneshot::channel();
        // Nothing has been sent yet, so the first request's turn comes at once.
        let _ = done.send(());
        Self {
            writer: Mutex::new(Writer { half: writer, tail }),
            reader: Mutex::new(reader),
            in_flight: AtomicUsize::new(0),
            broken: AtomicBool::new(false),
            reserved: AtomicBool::new(false),
            sent: StdMutex::new(VecDeque::new()),
        }
    }

    fn sent(&self) -> std::sync::MutexGuard<'_, VecDeque<String>> {
        self.sent
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn remember(&self, message_id: &str) {
        let mut sent = self.sent();
        if sent.len() == REMEMBERED {
            sent.pop_front();
        }
        sent.push_back(message_id.to_owned());
    }

    pub(super) async fn exchange(&self, message_id: &str) -> Outcome {
        let expected = message_id.trim_matches(['<', '>']);
        let mut turn = {
            let mut writer = self.writer.lock().await;
            if self.broken.load(Ordering::Acquire) {
                return Outcome::Collateral;
            }
            if let Err(error) = writer.half.send_body(message_id).await {
                self.broken.store(true, Ordering::Release);
                return Outcome::Broken(error);
            }
            self.remember(expected);
            let (done, tail) = oneshot::channel();
            let before_me = std::mem::replace(&mut writer.tail, tail);
            Turn {
                line: self,
                before_me,
                done: Some(done),
            }
        };
        // The answer before this one has to be off the line first.
        if (&mut turn.before_me).await.is_err() || self.broken.load(Ordering::Acquire) {
            return Outcome::Collateral;
        }
        let mut reader = self.reader.lock().await;
        let result = reader.read_body().await;
        drop(reader);
        match result {
            Ok(body) => {
                let named = match body.message_id {
                    None => Named::Nothing,
                    Some(answered) if answered == expected => Named::Requested,
                    Some(answered) => {
                        if self.sent().iter().any(|sent| *sent == answered) {
                            // Not finished on purpose: the turn's drop breaks the line.
                            return Outcome::Swapped { answered };
                        }
                        Named::Other(answered)
                    }
                };
                turn.finish();
                Outcome::Body {
                    data: body.data,
                    named,
                }
            }
            Err(BodyError::Unavailable(status)) => {
                let alone = self.in_flight.load(Ordering::Acquire) == 1;
                turn.finish();
                Outcome::Unavailable { status, alone }
            }
            // Not finished on purpose, like a break: the turn's drop marks the line, so a
            // request behind this one is told to ask again elsewhere instead of reading
            // from a connection the server is closing.
            Err(BodyError::ServerFault(status)) => Outcome::ServerFault { status },
            Err(BodyError::Broken(error)) => Outcome::Broken(error),
        }
    }
}

/// A request's turn to read. Dropped before [`Self::finish`], it breaks the line: the answer
/// it owed the line is still there, and the request behind it is told so through `done`.
struct Turn<'a> {
    line: &'a Line,
    before_me: oneshot::Receiver<()>,
    done: Option<oneshot::Sender<()>>,
}

impl Turn<'_> {
    fn finish(&mut self) {
        if let Some(done) = self.done.take() {
            // Nobody behind us is not an error.
            let _ = done.send(());
        }
    }
}

impl Drop for Turn<'_> {
    fn drop(&mut self) {
        if self.done.is_some() {
            self.line.broken.store(true, Ordering::Release);
        }
    }
}
