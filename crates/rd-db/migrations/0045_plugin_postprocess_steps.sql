-- Post-processing steps contributed by plugins (RD-090-16).
--
-- `checkpoint` is what a plugin step wrote when it last stopped: an opaque blob the core
-- stores and hands back verbatim on the next attempt, so a step interrupted by a restart
-- resumes instead of starting the package over. Only plugin steps write it; the built-in
-- stages resume by re-reading the files they work on.
ALTER TABLE postprocess_steps ADD COLUMN checkpoint BLOB NULL;

-- Per-category list of enabled plugin steps, as a JSON array of plugin ids in the order they
-- should run. NULL inherits the global list.
ALTER TABLE categories ADD COLUMN plugin_steps_json TEXT NULL;
