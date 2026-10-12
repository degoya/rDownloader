//! RD-1260-12: a plugin call leaves at most fifty lines in the log and one line about the rest;
//! the next call starts at zero, and a line the level filters out does not count.

use std::{
    fmt,
    sync::{Arc, Mutex},
};

use tracing::{
    Level, Subscriber,
    field::{Field, Visit},
};
use tracing_subscriber::{
    EnvFilter, Layer,
    layer::{Context, SubscriberExt},
    registry,
};

use crate::{
    PluginLimits, PluginStoreState, SandboxEngine, component::rdownloader::plugin::host::Host,
};

const PLUGIN: &str = "019d0000-0000-7000-8000-00000000abcd";

/// Every line that reached the layers behind the filter: its level and its message.
#[derive(Clone, Default)]
struct Lines(Arc<Mutex<Vec<(Level, String)>>>);

impl Lines {
    fn take(&self) -> Vec<(Level, String)> {
        std::mem::take(&mut *self.0.lock().expect("lines"))
    }
}

struct Message(String);

impl Visit for Message {
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        if field.name() == "message" {
            self.0 = format!("{value:?}");
        }
    }
}

impl<S: Subscriber> Layer<S> for Lines {
    fn on_event(&self, event: &tracing::Event<'_>, _context: Context<'_, S>) {
        let mut message = Message(String::new());
        event.record(&mut message);
        self.0
            .lock()
            .expect("lines")
            .push((*event.metadata().level(), message.0));
    }
}

fn listen(directives: &str) -> (Lines, tracing::subscriber::DefaultGuard) {
    let lines = Lines::default();
    let subscriber = registry()
        .with(EnvFilter::new(directives))
        .with(lines.clone());
    (lines, tracing::subscriber::set_default(subscriber))
}

fn store(plugin_id: &str) -> wasmtime::Store<PluginStoreState> {
    let sandbox = SandboxEngine::new(PluginLimits::default()).expect("sandbox");
    let mut store = sandbox.create_store(Vec::new()).expect("store");
    store.data_mut().set_log_origin(
        plugin_id.to_owned(),
        "0.4.2".to_owned(),
        "resolver",
        "resolve",
    );
    store
}

/// One call: `count` lines at `level`, then the store goes, as at the end of a call.
async fn call(plugin_id: &str, level: &str, count: usize) {
    let mut store = store(plugin_id);
    for line in 0..count {
        Host::log(store.data_mut(), level.to_owned(), format!("line {line}")).await;
    }
}

#[tokio::test]
async fn a_call_keeps_fifty_lines_and_names_how_many_more_it_dropped() {
    let (lines, _guard) = listen("rd_=info");

    call(PLUGIN, "info", 200).await;
    let written = lines.take();
    assert_eq!(written.len(), 51, "fifty lines and the warning");
    assert!(
        written[..50]
            .iter()
            .all(|(level, message)| *level == Level::INFO && message.starts_with("line ")),
        "{written:?}"
    );
    assert_eq!(written[49].1, "line 49");
    assert_eq!(
        written[50],
        (Level::WARN, "... 150 more lines suppressed".to_owned())
    );

    call(PLUGIN, "info", 10).await;
    assert_eq!(lines.take().len(), 10, "the next call starts at zero");
}

#[tokio::test]
async fn lines_the_level_filters_out_do_not_count() {
    let (lines, _guard) = listen("rd_=info");

    let mut store = store(PLUGIN);
    for line in 0..100 {
        Host::log(
            store.data_mut(),
            "debug".to_owned(),
            format!("debug {line}"),
        )
        .await;
    }
    for line in 0..50 {
        Host::log(store.data_mut(), "info".to_owned(), format!("info {line}")).await;
    }
    drop(store);

    let written = lines.take();
    assert_eq!(
        written.len(),
        50,
        "fifty info lines and no warning: {written:?}"
    );
    assert!(written.iter().all(|(level, _)| *level == Level::INFO));
}

/// The span the host opens around a line is the one a debug window on one plugin names.
#[tokio::test]
async fn a_debug_window_on_one_plugin_reaches_only_its_lines() {
    let directives = format!(
        "rd_=info,[{}{{plugin_id={PLUGIN}}}]=debug",
        rd_core::PLUGIN_LOG_SPAN
    );
    let (lines, _guard) = listen(&directives);

    call(PLUGIN, "debug", 3).await;
    call("019d0000-0000-7000-8000-00000000ffff", "debug", 3).await;

    assert_eq!(lines.take().len(), 3, "only the named plugin's debug lines");
}
