//! The "accepting unsigned development plugin" warning: once per plugin version and process.
//!
//! A package is verified again wherever it is read — at start by the bundled sync, the provider
//! refresh and the load, afterwards by every manifest listing and every locale bundle the
//! interface asks for. In development mode each of those passes warned once per unsigned
//! plugin, so a start with the bundled plugins unsigned wrote the same line hundreds of times,
//! and CI's `components` job drowned in it (RD-140-25). It stays a WARN, since it is the one
//! sign that code no key vouches for is running; but each plugin version is named once per
//! process, and a bulk pass names all of its new ones in a single line when it ends.

use std::{
    collections::BTreeSet,
    sync::{Mutex, MutexGuard, PoisonError},
};

static NOTICES: Mutex<UnsignedNotices> = Mutex::new(UnsignedNotices::new());

/// What to log, decided apart from the logging so it can be tested without a subscriber.
#[derive(Debug, PartialEq, Eq)]
enum Notice {
    One(String),
    Summary(Vec<String>),
}

#[derive(Debug)]
struct UnsignedNotices {
    /// Every unsigned plugin version accepted so far in this process.
    seen: BTreeSet<String>,
    /// Bulk passes running now; while one is, new names wait for its end.
    open_batches: usize,
    pending: BTreeSet<String>,
}

impl UnsignedNotices {
    const fn new() -> Self {
        Self {
            seen: BTreeSet::new(),
            open_batches: 0,
            pending: BTreeSet::new(),
        }
    }

    fn accept(&mut self, plugin: String) -> Option<Notice> {
        if !self.seen.insert(plugin.clone()) {
            return None;
        }
        if self.open_batches == 0 {
            return Some(Notice::One(plugin));
        }
        self.pending.insert(plugin);
        None
    }

    fn open(&mut self) {
        self.open_batches += 1;
    }

    fn close(&mut self) -> Option<Notice> {
        self.open_batches = self.open_batches.saturating_sub(1);
        if self.open_batches > 0 || self.pending.is_empty() {
            return None;
        }
        Some(Notice::Summary(
            std::mem::take(&mut self.pending).into_iter().collect(),
        ))
    }
}

fn notices() -> MutexGuard<'static, UnsignedNotices> {
    // A panic elsewhere while holding the lock leaves the sets consistent; a lost warning
    // would not be.
    NOTICES.lock().unwrap_or_else(PoisonError::into_inner)
}

fn emit(notice: Option<Notice>) {
    match notice {
        Some(Notice::One(plugin)) => {
            tracing::warn!(%plugin, "accepting unsigned development plugin");
        }
        Some(Notice::Summary(plugins)) => tracing::warn!(
            count = plugins.len(),
            plugins = %plugins.join(", "),
            "accepted unsigned development plugins"
        ),
        None => {}
    }
}

/// Records that `name` `version` was accepted without a signature, warning if it is new.
pub(crate) fn accepted(name: &str, version: &str) {
    let notice = notices().accept(format!("{name} {version}"));
    emit(notice);
}

/// Held for the length of a pass over many packages; the new names it accepted are warned
/// about together when the last open pass ends, including when it ends by an error.
pub(crate) struct UnsignedBatch(());

impl UnsignedBatch {
    pub(crate) fn open() -> Self {
        notices().open();
        Self(())
    }
}

impl Drop for UnsignedBatch {
    fn drop(&mut self) {
        let notice = notices().close();
        emit(notice);
    }
}

#[cfg(test)]
mod tests {
    use super::{Notice, UnsignedNotices};

    #[test]
    fn a_single_verification_warns_at_once_and_only_once() {
        let mut notices = UnsignedNotices::new();
        assert_eq!(
            notices.accept("demo 1.0.0".to_owned()),
            Some(Notice::One("demo 1.0.0".to_owned()))
        );
        assert_eq!(notices.accept("demo 1.0.0".to_owned()), None);
        // Another version is another package.
        assert_eq!(
            notices.accept("demo 1.1.0".to_owned()),
            Some(Notice::One("demo 1.1.0".to_owned()))
        );
    }

    #[test]
    fn a_pass_names_its_new_plugins_in_one_line_when_the_last_one_ends() {
        let mut notices = UnsignedNotices::new();
        assert!(notices.accept("seen 1.0.0".to_owned()).is_some());
        notices.open();
        notices.open();
        for plugin in ["zeta 1.0.0", "alpha 1.0.0", "seen 1.0.0", "alpha 1.0.0"] {
            assert_eq!(notices.accept(plugin.to_owned()), None);
        }
        assert_eq!(notices.close(), None, "an inner pass does not flush");
        assert_eq!(
            notices.close(),
            Some(Notice::Summary(vec![
                "alpha 1.0.0".to_owned(),
                "zeta 1.0.0".to_owned()
            ]))
        );
        // The next pass over the same packages has nothing new to say.
        notices.open();
        assert_eq!(notices.accept("zeta 1.0.0".to_owned()), None);
        assert_eq!(notices.close(), None);
    }
}
