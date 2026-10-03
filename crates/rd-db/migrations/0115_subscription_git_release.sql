-- RD-190-13: which release assets a git-release subscription downloads (platforms,
-- architectures, name patterns, pre-releases, source archives), as a JSON object. Every
-- existing subscription gets the empty object, which no other kind reads.
ALTER TABLE subscriptions ADD COLUMN git_release_json TEXT NOT NULL DEFAULT '{}';
