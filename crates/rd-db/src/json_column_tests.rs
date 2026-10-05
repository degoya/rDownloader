//! A broken JSON column still reads as its default, and now says so (audit Q1).

use std::{
    fmt::Debug,
    sync::{Arc, Mutex},
};

use tracing::{
    Event, Metadata, Subscriber,
    field::{Field, Visit},
    span::{Attributes, Id, Record},
};

use super::lenient;

/// Collects the fields of every event as `name=value` text; no tracing-subscriber needed.
#[derive(Clone, Default)]
struct Recorder(Arc<Mutex<Vec<String>>>);

struct Fields(String);

impl Visit for Fields {
    fn record_debug(&mut self, field: &Field, value: &dyn Debug) {
        self.0.push_str(&format!("{}={value:?} ", field.name()));
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        self.0.push_str(&format!("{}={value} ", field.name()));
    }
}

impl Subscriber for Recorder {
    fn enabled(&self, _: &Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, _: &Attributes<'_>) -> Id {
        Id::from_u64(1)
    }
    fn record(&self, _: &Id, _: &Record<'_>) {}
    fn record_follows_from(&self, _: &Id, _: &Id) {}
    fn event(&self, event: &Event<'_>) {
        let mut fields = Fields(format!("{} ", event.metadata().level()));
        event.record(&mut fields);
        self.0.lock().expect("recorder").push(fields.0);
    }
    fn enter(&self, _: &Id) {}
    fn exit(&self, _: &Id) {}
}

fn recorded<T>(read: impl FnOnce() -> T) -> (T, Vec<String>) {
    let recorder = Recorder::default();
    let value = tracing::subscriber::with_default(recorder.clone(), read);
    let lines = recorder.0.lock().expect("recorder").clone();
    (value, lines)
}

#[test]
fn a_readable_value_is_returned_without_a_warning() {
    let (value, lines) = recorded(|| {
        lenient::<Vec<String>>(
            serde_json::from_str(r#"["a","b"]"#),
            "categories",
            "x",
            "id-1",
        )
    });
    assert_eq!(value, Some(vec!["a".to_owned(), "b".to_owned()]));
    assert!(lines.is_empty(), "{lines:?}");
}

#[test]
fn an_unreadable_value_reads_as_none_and_names_table_column_and_row() {
    let (value, lines) = recorded(|| {
        lenient::<Vec<String>>(
            serde_json::from_str("{not json"),
            "categories",
            "seeding_json",
            "0b6c-row",
        )
    });
    assert_eq!(value, None);
    assert_eq!(lines.len(), 1, "{lines:?}");
    let line = &lines[0];
    assert!(line.starts_with("WARN "), "{line}");
    for expected in [
        "table=categories",
        "column=seeding_json",
        "row=0b6c-row",
        "error=",
    ] {
        assert!(line.contains(expected), "{expected} missing in {line}");
    }
}

#[test]
fn a_value_of_the_wrong_shape_is_reported_like_broken_text() {
    let (value, lines) = recorded(|| {
        lenient::<u64>(
            serde_json::from_value(serde_json::json!("many")),
            "settings",
            "value_json",
            "mirror.preference",
        )
    });
    assert_eq!(value, None);
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(lines[0].contains("row=mirror.preference"), "{}", lines[0]);
}
