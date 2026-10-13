-- RD-1280-02: a notification rule's own title and text, with `{placeholder}` holes the service
-- fills when it queues a delivery. NULL sends the built-in wording, as every rule did before.
ALTER TABLE notification_rules ADD COLUMN title_template TEXT;
ALTER TABLE notification_rules ADD COLUMN body_template TEXT;
