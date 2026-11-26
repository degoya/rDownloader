pub fn message_of(delivery: &Delivery) -> Message {
    let mut payload = serde_json::json!({
        "event": delivery.event,
        "title": delivery.title,
    });
}
