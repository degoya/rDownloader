# External tool compatibility

rDownloader runs a handful of programs it does not ship: yt-dlp, gallery-dl, Streamlink,
FFmpeg and ffprobe do the work for media, galleries and recordings, and unrar, 7-Zip and
rclone do the work after it. Their versions move on their own schedule, which means an
installation can be perfectly configured and still fail because the binary on disk predates
something rDownloader asks it to do.

This page states which versions are supported, what happens when one is not, and where the
platforms differ. Read the last section before treating anything here as a guarantee.

## The four verdicts

A verdict is not a boolean, because the four answers call for different actions.

| Verdict | Meaning | Effect |
| --- | --- | --- |
| `supported` | At or above the floor and not on a known-bad list | Nothing |
| `too_old` | Below the floor this build is tested against | Blocks the named capabilities |
| `known_bad` | A specific release listed as broken | Blocks the named capabilities |
| `unknown` | The version could not be read, or no rule covers the tool | **Never blocks** |

`unknown` is the important one. "We could not read the version" is not evidence of a fault:
FFmpeg git builds print `N-113522-g8b0a3d5c`, which carries no number to compare, and a build
like that is usually newer than every release rather than older. Treating an unreadable
version as an old one would turn every unusual build into a broken installation, so it warns
and gates nothing.

## What a verdict costs

A rule names the capabilities it affects, and only those are blocked. Nothing here can refuse
work in general: a yt-dlp below the floor stops media downloads while HTTP transfers, the
Usenet queue, torrents and everything else keep running.

| Capability | What stops | Tools it depends on |
| --- | --- | --- |
| `media_download` | Probing and downloading media | yt-dlp |
| `media_merge` | Merging a separate video and audio stream | ffmpeg **and** ffprobe |
| `audio_extraction` | Converting to MP3 | ffmpeg **and** ffprobe |
| `gallery_download` | Downloading an image gallery | gallery-dl |
| `stream_recording` | Recording a live stream | streamlink |

A blocked job fails with the stable code `media.tool_incompatible`, carrying the tool, the
version, the minimum and the capability as parameters. The interface translates the capability
into the reader's language; a capability shown as a bare identifier would be no better than no
name at all.

## The rules shipped with this release

Each floor is the oldest version this project tests against, not a guess at where a tool
broke.

| Tool | Minimum | Known bad | Capabilities gated |
| --- | --- | --- | --- |
| `yt-dlp` | 2023.01.06 | — | `media_download` |
| `gallery-dl` | 1.25.0 | — | `gallery_download` |
| `streamlink` | 5.5.0 | — | `stream_recording` |
| `ffmpeg` | 4.4 | — | `media_merge`, `audio_extraction` |
| `ffprobe` | 4.4 | — | `media_merge`, `audio_extraction` |

`unrar`, `7z` and `rclone` carry no rule. No capability here is derived from their version, and
their own failures are already reported by the tools themselves. `par2` is absent because
repair runs in-process through `rust_par2`, with no binary to check at all.

**Nothing is shipped as known bad.** Withdrawing a specific release is a statement about the
world after this build was made, so it belongs in the signed manifest rather than frozen into
a binary. The mechanism is there and is covered by tests; the shipped list is empty on
purpose.

## The builds shipped with this release

The manifest compiled into the release names concrete builds, with the SHA-256 that decides
whether the bytes are accepted. Every hash and size below was produced by downloading the
asset and hashing it, not copied from a release page.

| Tool | Version | Platform | Source | Archive |
| --- | --- | --- | --- | --- |
| `yt-dlp` | 2026.08.19 | `x86_64-unknown-linux-gnu` | yt-dlp GitHub release, `yt-dlp_linux` | raw |
| `yt-dlp` | 2026.08.19 | `aarch64-unknown-linux-gnu` | yt-dlp GitHub release, `yt-dlp_linux_aarch64` | raw |
| `yt-dlp` | 2026.08.19 | `x86_64-pc-windows-msvc` | yt-dlp GitHub release, `yt-dlp.exe` | raw |
| `yt-dlp` | 2026.08.19 | `aarch64-pc-windows-msvc` | yt-dlp GitHub release, `yt-dlp_arm64.exe` | raw |
| `ffmpeg`, `ffprobe` | 9.0.1 | `x86_64-unknown-linux-gnu` | BtbN/FFmpeg-Builds, `autobuild-2026-09-07-15-39` | `tar.xz` |
| `ffmpeg`, `ffprobe` | 9.0.1 | `x86_64-pc-windows-msvc` | BtbN/FFmpeg-Builds, `autobuild-2026-09-07-15-39` | `zip` |

`ffmpeg` and `ffprobe` are two managed tools inside one archive: two manifest entries with the
same URL, size and hash, each naming its own member. Installing both downloads the archive
twice, which is the price of keeping one entry equal to one managed binary.

**FFmpeg comes from BtbN, a third party, not an official FFmpeg distribution.** RD-102-02's
*Out of scope* excludes "Tools ohne nachvollziehbare offizielle Distribution", and this is a
deliberate, user-approved deviation from it: the FFmpeg project publishes no binaries at all,
so an official distribution is not something that exists to be preferred. BtbN's builds are
the ones FFmpeg's own download page points Windows and Linux users to, they are reproducible
from a public workflow, and — like every other entry — they are pinned by hash, so the
deviation is about who compiled the bytes, not about whether the bytes are checked.

The BtbN entry is pinned to the **immutable dated release** `autobuild-2026-09-07-15-39`
rather than to the `latest` tag. `latest` is a moving target: it is re-uploaded on every
rebuild, so a hash pinned against it would go stale within days and every install would then
fail with `tools.hash_mismatch`. A dated release keeps its bytes.

### What is not managed, and why

Two of the five managed tool names ship no entries. They are still managed tools — the name is
accepted, the store would hold them — but the manifest offers no build, so they resolve
through the vendor folders and `PATH` exactly as before.

* **gallery-dl** publishes to PyPI, not as release assets. Its last eight GitHub releases carry
  zero attached files, so there is no per-platform binary to pin a hash against.
* **Streamlink** publishes no standalone Linux binary at all, and its Windows bundle from
  `streamlink/windows-builds` is a 2471-entry ZIP carrying a complete Python runtime (2426
  entries under `pkgs/`, 36 under `Python/`). The `members` model lands every extracted member
  under its own file name in one flat version directory, which would flatten that tree into
  colliding names and produce a broken tool. Managing it needs structure-preserving
  extraction, which is a separate piece of work rather than a manifest entry.

Nothing here is a defect in the tools. Both remain fully supported when they are installed by
other means; only the *managed* half is absent.

## Where the rules come from

Rules travel inside the signed tool manifest, alongside the builds they talk about. That means
one signature, one domain separator and one replay floor for both — a rule cannot be replaced
without replacing the manifest, and a manifest cannot be replayed to bring an old rule back.
The manifest compiled into the release is the offline-safe floor; a configured `https://` URL
supplies newer ones.

Any failure to read a delivered rule set leaves the compiled-in base in force, and says so in
the log. That covers an unusable signature, a stale document, a rule about a tool this build
never looks up, a version string that does not parse, a rule that names no capability, and a
set larger than this build reads. The whole set is dropped rather than the offending rule:
a document that cannot be read completely is not one to act on partially.

A delivered rule replaces the compiled-in rule for the tool it names and leaves the rest
standing, so a manifest with something to say about yt-dlp cannot silently drop the FFmpeg
floor.

**The manifest shipped with this release delivers no rules**, deliberately, even though it now
delivers builds. A delivered rule replaces the compiled-in one wholesale, so repeating the
floors in the signed document would freeze today's numbers into it: raising a floor in the code
would then be silently overridden by the manifest compiled beside it. An empty rule set leaves
the table above in force and keeps one source of truth. The field is there for the case it was
built for — withdrawing a specific release after this build was made — and that is a
publishing decision, not a shipped default.

## Overriding a rule

**Settings → Tools → Override compatibility rules** takes the tools whose verdict should
be reported but not enforced. The warning stays, the badge stays, and only the block goes.
Every evaluation that skipped a block writes a `tracing` record naming the tool, the verdict,
the version, the floor and the capabilities involved, so an override is visible afterwards
rather than only at the moment it was set. A tool with no rule behind it is refused when the
settings are saved, because storing it would look like it did something.

## Reading the version

`rdownloader doctor` prints the path, the source, the version, the verdict and — when there is
something to do — the upgrade path, per tool. The same information is in
**Settings → Tools** and in `GET /api/v1/system/media`, under each tool's `compatibility`
block.

The version is read by running the binary once and is cached against that file's modification
time and size. Replacing a binary, upgrading a system package or activating another managed
version all invalidate the entry on their own.

## Platform differences

The lookup order — explicit setting, managed store, vendor folders, `PATH` — is the same
everywhere. What differs is how the binaries are named, packaged and asked.

| Aspect | Linux | macOS | Windows |
| --- | --- | --- | --- |
| Executable name | `yt-dlp` | `yt-dlp` | `yt-dlp` or `yt-dlp.exe`, both probed |
| Managed versions | Yes, per target triple | No — the embedded manifest carries no `*-apple-darwin` entry, so every tool resolves through the vendor folders or `PATH` | Yes, per target triple |
| Version probe window | — | — | Spawned with `CREATE_NO_WINDOW`, so no console flashes |
| FFmpeg version string | Usually a distribution build, e.g. `6.1.1-3ubuntu5` | Homebrew builds report a plain triple | BtbN builds report `n<version>-…`, e.g. `n9.0.1-27-g…`, which parses to the release number; gyan.dev git-master builds report `N-…-g…` git descriptions, which read as `unknown` |
| Streamlink | `streamlink` on `PATH` or in a vendor folder | same | additionally the portable build at `vendor/streamlink/bin/streamlink.exe` |
| Activation | Pointer file, no symlink | — (no managed versions to activate) | Pointer file, because symlinks need a privilege |

Two consequences are worth stating plainly:

* **A gyan.dev git-master FFmpeg on Windows is `unknown` rather than `supported`**, because such
  a build names no release. That is the correct answer — those builds are newer than every
  release — and it blocks nothing. A BtbN build carries its release number and is compared
  like any other.
* **A distribution suffix is not a pre-release.** `6.1.1-3ubuntu5` is 6.1.1, not something
  below it, so a floor of 6.1.1 is reachable on Debian and Ubuntu. Only a named marker (`rc`,
  `beta`, `dev`, `alpha`, `pre`, `nightly`, `snapshot`) sorts a build below the release it
  names.

## What has actually been verified

* The parser matrix — valid release, pre-release, nightly, distribution suffix, git
  description, garbage, empty output and a line from a different program — runs as unit tests
  in `crates/rd-tools/src/version.rs` on every platform CI builds.
* Spawning a real process, the timeout, and the cache are covered in
  `crates/rd-tools/tests/tool_compatibility.rs` against shell-script fixtures, so those cases
  run on Linux and macOS and not on Windows.
* The end-to-end gate — an unsupported yt-dlp refusing a media probe with the capability named
  — is covered in `crates/rd-media/tests/fake_ytdlp.rs`, also with a shell fixture and
  therefore also Unix-only.
* The Windows column above is derived from how the lookup and the spawn are written, not from
  a run against every listed build. Treat it as documentation of intent where it is not
  covered by a test.
* The manifest's own entries are checked by `every_entry_of_the_embedded_manifest_validates`
  and `the_embedded_manifest_covers_the_platforms_the_documentation_claims` in
  `crates/rd-tools/src/manifest.rs`, so the table above cannot drift away from the signed
  document without a test failing.
* Unpacking is covered against archives the tests build themselves — a member that lands flat
  under its base name, a member path that tries to climb out of the staging directory, a
  corrupt and a truncated xz stream, and the unpack budget — in
  `crates/rd-tools/src/download.rs` and `crates/rd-tools/tests/managed_tools.rs`.
* **No install has been run against the live URLs.** The hashes and sizes were produced by
  downloading each asset once and hashing it, and the Linux FFmpeg binary was extracted from
  its real archive and executed (`ffmpeg version n9.0.1-27-g9b0578816c-20260907`). What has
  not been exercised is the application performing that download itself, on any platform. The
  path is covered by tests against a local server; the live combination of these URLs with
  this code is not.
