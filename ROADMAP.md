# Roadmap

Where rDownloader is heading after 1.2. This is a plan, not a promise: milestones move, change
shape or split when daily use shows something more urgent, and nothing here is a date. What has
shipped is in [`CHANGELOG.md`](CHANGELOG.md).

## 1.3 — Everyday use

What daily use asked for, in small pieces: faster plugin start-up, the site rules as a signed file
of their own, an About page, clearing the notification history, a configurable sign-in lifetime,
hiding hosters in the LinkGrabber, and the browser extension handing over bytes only the browser
could load. Plus faster builds and checks, which make every later milestone cheaper.

## 1.4 — Plugin distribution

A hoster fix should reach users without a full application release. Signed plugin repositories
with an offline cache, permissions shown before an install, staged plugin versions with atomic
rollback — and a security review of plugin trust before any of it ships.

## 1.5 — Storage intelligence and object storage

Configurable collision policies, duplicate detection by content hash, hardlink or reflink
deduplication and verified moves. Metalink with several mirrors per file, and S3-compatible object
storage as a download source and upload destination.

## 1.6 — Backup and disaster recovery

Scheduled, encrypted backups of the configuration and the database, including queue state,
torrent sessions and partial transfers, to local, WebDAV, S3-compatible or rclone-backed
destinations, with retention, integrity checks and a restore preview. The release that closes
1.6 is the first public release.

## 1.7 — Bug fixes and hardening

The round right after going public, with no new features on purpose: what public users report,
a security review of the browser-capture, archive-extraction and user-script boundaries,
forced-termination and long-running tests, compatibility gates for the REST API and the plugin
ABI, and a support matrix built from tested data.

## 1.8 — Installer, self-update, signing

Signed in-app updates with release channels, atomic installation with rollback and a backup
before every schema change, installer packages, native Linux arm64 builds, and signed Windows and
macOS releases — the last of which depends on purchased certificates.
