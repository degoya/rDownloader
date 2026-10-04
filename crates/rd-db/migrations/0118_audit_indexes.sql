-- DB-10 (code audit 1.9.1): indexes for six columns that are looked up or cascaded on and had
-- none, so each of these lookups read its whole table.
--
-- content_index(path): every finished download deletes its entry by path.
-- notification_deliveries(rule_id): the per-rule trim and the rule's history.
-- collector_packages(batch_id): the batch cascade and the batch's packages.
-- automation_runs(automation_version_id): the cascade when a version goes.
-- plugin_repository_installs(repository_id): the cascade when a repository goes.
-- object_uploads(profile_id): the cascade when a storage profile goes.
CREATE INDEX content_index_path_idx ON content_index(path);
CREATE INDEX notification_deliveries_rule_idx ON notification_deliveries(rule_id);
CREATE INDEX collector_packages_batch_idx ON collector_packages(batch_id);
CREATE INDEX automation_runs_version_idx ON automation_runs(automation_version_id);
CREATE INDEX plugin_repository_installs_repository_idx
    ON plugin_repository_installs(repository_id);
CREATE INDEX object_uploads_profile_idx ON object_uploads(profile_id);
