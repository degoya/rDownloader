# Release notes

What each rDownloader version changes for you, in a few plain points. The update dialog, the
Updates page and the GitHub release show them; every detail, for developers too, is in
[CHANGELOG.md](CHANGELOG.md).

<!--
How a section is written (scripts/release-notes.sh --check holds the rules): "## X.Y.Z", newest
first, then one to eight "- " points in English of what a user notices: what is new, what works
better, what is fixed. At most 200 characters each, ending with a full stop. No job numbers, no
paths, no code, no internals (release pipeline, tests, refactoring). A version without anything a
user notices says only: Maintenance release: internal changes only, no change in behaviour.
-->

## 1.15.0

- When a new version is available, the Updates page now says so at the top, with its highlights and a button to install it.
- What's new in an update is now described in plain words, with links to the full changes and to the release page.
- Long release names in a subscription's entries no longer push its actions off the screen.
- Any subscription entry can be queued again, including one you discarded by mistake.
- Indexer subscriptions no longer miss entries when many new ones arrive between two checks.
