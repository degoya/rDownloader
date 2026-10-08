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

## 1.18.0

- Packages in the Downloads and the LinkGrabber stay open or closed across reloads, open or close all at once, and start closed or open as you set it.
- A site rule can now split a page with several releases into one package per release, with the same file at different hosters shown as mirrors.
- hide.cx, warez.cx and serienjunkies.org pages are now recognised by the rules that come with rDownloader.
- A series page such as serienjunkies.org now lists its releases by season in the LinkGrabber first: pick the seasons, episodes and qualities you want, and only those are fetched, one captcha each.
- The capture agent can pause clipboard watching from its tray, a shortcut or the settings, so copied links stay out of rDownloader until you switch it back on.
- Every tray command of the capture agent has a system-wide keyboard shortcut you set yourself, and a new command hands the clipboard over once, even while watching is paused.
- Long pick lists of your own categories, accounts, proxies, profiles and the like now have a search field: type part of a name and press Enter.

## 1.16.1

- Click'n'Load from hide.cx now reaches the LinkGrabber.

## 1.16.0

- The settings pages for notifications, backup and restore, About, remote transfers and media are now split into tabs, like the other long settings pages.
- The search in the settings opens the right tab on these pages, and a half-filled form survives a switch between tabs.

## 1.15.0

- When a new version is available, the Updates page now says so at the top, with its highlights and a button to install it.
- What's new in an update is now described in plain words, with links to the full changes and to the release page.
- Long release names in a subscription's entries no longer push its actions off the screen.
- Any subscription entry can be queued again, including one you discarded by mistake.
- Indexer subscriptions no longer miss entries when many new ones arrive between two checks.
