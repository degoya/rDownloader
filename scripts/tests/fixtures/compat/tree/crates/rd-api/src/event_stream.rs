const LAGGED_EVENT: &str = "stream.lagged";

fn event_name(kind: &EventKind) -> &'static str {
    match kind {
        EventKind::ItemChanged => "item.changed",
        EventKind::System => "system",
    }
}
