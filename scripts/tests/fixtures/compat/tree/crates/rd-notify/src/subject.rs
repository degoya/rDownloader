impl Subject {
    pub fn describe(&self, body: &mut serde_json::Value) {
        body["version"] = PAYLOAD_VERSION.into();
    }
}
