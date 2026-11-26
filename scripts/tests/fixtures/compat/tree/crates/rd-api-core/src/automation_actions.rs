fn automation_message(text: Option<&str>) -> rd_notify::Message {
    let mut payload = serde_json::json!({
        "event": "automation",
    });
    if let Some(text) = text {
        payload["message"] = serde_json::Value::from(text);
    }
}

fn keep() -> ApiError {
    ApiError::conflict("item.kept", "The item was kept")
}
