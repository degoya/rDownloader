-- RD-150-08: the arguments a script subscription hands its script.
--
-- A JSON array of strings, each passed to the process as one argument of its own -- no shell
-- splits or expands them. Every existing subscription gets the empty list, which is what every
-- script ran with before this column. Only a script subscription carries arguments; the
-- column is general for the same reason `schedule` is.
ALTER TABLE subscriptions ADD COLUMN script_arguments_json TEXT NOT NULL DEFAULT '[]';
