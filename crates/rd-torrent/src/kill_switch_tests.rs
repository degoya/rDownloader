//! A pause or resume the engine refuses is logged and tried again at the next check
//! (RD-1120-04, audit S1). Each call of `hold_torrents`/`resume_held` here is one check of the
//! interface watcher; the torrents are stand-ins whose engine answers a test decides.

use std::{
    collections::HashSet,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

use async_trait::async_trait;
use rd_core::DownloadId;

use super::{Switched, hold_torrents, resume_held};

/// A torrent whose next `refusals` pauses or resumes fail.
struct Stand {
    paused: AtomicBool,
    refusals: AtomicUsize,
}

impl Stand {
    fn new(paused: bool, refusals: usize) -> Self {
        Self {
            paused: AtomicBool::new(paused),
            refusals: AtomicUsize::new(refusals),
        }
    }

    fn set_paused(&self, paused: bool) -> anyhow::Result<()> {
        if self.refusals.load(Ordering::SeqCst) > 0 {
            self.refusals.fetch_sub(1, Ordering::SeqCst);
            anyhow::bail!("the engine refused");
        }
        self.paused.store(paused, Ordering::SeqCst);
        Ok(())
    }
}

#[async_trait]
impl Switched for Stand {
    fn info_hash(&self) -> String {
        "c0ffee".to_owned()
    }

    fn carries_traffic(&self) -> bool {
        !self.paused.load(Ordering::SeqCst)
    }

    fn paused(&self) -> bool {
        self.paused.load(Ordering::SeqCst)
    }

    async fn pause(&self) -> anyhow::Result<()> {
        self.set_paused(true)
    }

    async fn resume(&self) -> anyhow::Result<()> {
        self.set_paused(false)
    }
}

#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Captured {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().expect("log buffer").extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Captured {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().expect("log buffer")).into_owned()
    }
}

fn capturing_subscriber(sink: &Captured) -> impl tracing::Subscriber + Send + Sync {
    let sink = sink.clone();
    tracing_subscriber::fmt()
        .with_ansi(false)
        .with_max_level(tracing::Level::TRACE)
        .with_writer(move || sink.clone())
        .finish()
}

#[tokio::test]
async fn kill_switch_retries_a_failed_pause() {
    let sink = Captured::default();
    let _guard = tracing::subscriber::set_default(capturing_subscriber(&sink));
    let (refusing, willing) = (DownloadId::new(), DownloadId::new());
    let torrents = [
        (refusing, Stand::new(false, 1)),
        (willing, Stand::new(false, 0)),
    ];
    let mut held = HashSet::new();

    hold_torrents(&torrents, &mut held, "kill switch").await;
    assert!(torrents[0].1.carries_traffic(), "the refused pause took");
    assert!(!torrents[1].1.carries_traffic());
    assert_eq!(held, HashSet::from([willing]));
    let log = sink.text();
    assert_eq!(
        log.matches("a torrent could not be paused").count(),
        1,
        "{log}"
    );
    assert!(log.contains(&format!("download_id={refusing}")), "{log}");
    assert!(log.contains("info_hash=c0ffee"), "{log}");
    assert!(log.contains("the engine refused"), "{log}");

    hold_torrents(&torrents, &mut held, "kill switch").await;
    assert!(
        !torrents[0].1.carries_traffic(),
        "the next check did not pause it"
    );
    assert_eq!(held, HashSet::from([refusing, willing]));
    let log = sink.text();
    assert_eq!(
        log.matches("a torrent could not be paused").count(),
        1,
        "{log}"
    );
}

#[tokio::test]
async fn kill_switch_resumes_what_it_paused_and_retries_a_failed_resume() {
    let sink = Captured::default();
    let _guard = tracing::subscriber::set_default(capturing_subscriber(&sink));
    let (refusing, willing, stopped, gone) = (
        DownloadId::new(),
        DownloadId::new(),
        DownloadId::new(),
        DownloadId::new(),
    );
    let torrents = [
        (refusing, Stand::new(true, 1)),
        (willing, Stand::new(true, 0)),
        // Paused by the user, not by the switch: the release leaves it alone.
        (stopped, Stand::new(true, 0)),
    ];
    let mut held = HashSet::from([refusing, willing, gone]);

    resume_held(&torrents, &mut held, "kill switch").await;
    assert!(torrents[0].1.paused(), "the refused resume took");
    assert!(!torrents[1].1.paused());
    assert!(
        torrents[2].1.paused(),
        "a torrent the switch never paused was resumed"
    );
    assert_eq!(held, HashSet::from([refusing]), "{held:?}");
    let log = sink.text();
    assert_eq!(
        log.matches("a paused torrent could not be resumed").count(),
        1,
        "{log}"
    );
    assert!(log.contains(&format!("download_id={refusing}")), "{log}");

    resume_held(&torrents, &mut held, "kill switch").await;
    assert!(!torrents[0].1.paused(), "the next check did not resume it");
    assert!(torrents[2].1.paused());
    assert!(held.is_empty(), "{held:?}");
}
