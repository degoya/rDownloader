# Download-client compatibility

rDownloader can act as a download client for automation tools written against SABnzbd and
qBittorrent. Both adapters are translations at the edge: they reuse the native handlers,
validation and removal logic underneath, and neither can reach anything the native API does
not already expose.

This page states what is supported, what is deliberately not, and what has actually been
verified. Read the last section before treating anything here as a guarantee.

## Setting it up

1. Create an API token under **Settings → MCP**. It carries
   the `api:*` scope. A read-only token does not work: these adapters add and delete.
2. In your automation tool, add a download client:
   - **SABnzbd** — host and port of rDownloader, no URL base (or `sabnzbd` if the tool
     insists on one), and the token as the API key.
   - **qBittorrent** — host and port, any username, and the token as the password.
3. Create the category your tool uses — `tv-sonarr`, `movies-radarr` — under **Settings →
   Storage & rules**, with the same name. The tool asks rDownloader to create it on every
   connect and that request is accepted without effect, so the category has to exist here for
   the tool's own queue filter to find what it queued.

The two can be used at the same time from different tools against one installation.

## SABnzbd adapter

Served at `/api` and `/sabnzbd/api`. Authenticated with `apikey=` in the query or an
`X-Api-Key` header. Failures answer `HTTP 200` with `{"status": false, "error": …}`, which is
SABnzbd's own convention — such clients read the body and treat a non-200 as an outage.

| Mode | Status | Notes |
| --- | --- | --- |
| `version` | Supported | Reports a SABnzbd version whose API this subset matches |
| `auth` | Supported | Always `apikey` |
| `get_config` | Supported | `complete_dir` and the category list only |
| `get_cats` | Supported | `*` plus the configured categories |
| `fullstatus`, `status` | Supported | Reachability probe |
| `queue` | Supported | Packages that have not reached an end state |
| `queue&name=delete` | Supported | Single id or `value=all`; cancels and cleans up part files |
| `queue&name=pause`, `name=resume` | Supported | Applies to every download of the package |
| `pause`, `resume` | Supported | Everything not finished |
| `history` | Supported | Completed and failed packages, with `storage` |
| `history&name=delete` | Supported | Comma-separated ids or `all` |
| `addfile` | Supported | Multipart NZB; queued immediately, not held for review |
| `addurl` | **Refused** | See *Deliberate departures* |
| `addlocalfile` | **Refused** | Answered like `addurl`: the message points at `mode=addfile` |
| `del_files=1` | **Ignored** | See *Deliberate departures* |
| Everything else | Reports failure | `{"status": false, "error": "not implemented: …"}` |

## qBittorrent adapter

Served at `/api/v2`, following qBittorrent's Web API v2. `POST /api/v2/auth/login` answers
`Ok.` or `Fails.` and sets a `SID` cookie; the cookie carries the API token itself, so there
is no session to expire and revoking the token takes effect on the next call. An
unauthenticated call answers `403`, which clients read as "log in again".

| Endpoint | Status | Notes |
| --- | --- | --- |
| `auth/login`, `auth/logout` | Supported | Username ignored; password is the API token |
| `app/version`, `app/webapiVersion` | Supported | |
| `app/preferences` | Supported | `save_path`, plus fixed values for `temp_path`, `temp_path_enabled`, `create_subfolder_enabled`, `preallocate_all`, the queueing switches, `dht`, `pex`, `lsd` and `listen_port` |
| `torrents/info` | Supported | `category` and `filter=completed\|downloading` narrowing; the package's category is reported and matched the way qBittorrent does it — no parameter means any category, an empty one means the torrents without one |
| `torrents/properties` | Supported | |
| `torrents/files` | Supported | Resolved through the same evaluator as the UI |
| `torrents/filePrio` | Partial | Priority `0` deselects, anything else selects |
| `torrents/add` | Supported | `.torrent` upload and magnets; queued immediately |
| `torrents/delete` | Supported | `deleteFiles=true` does not delete finished files |
| `torrents/pause`, `stop` | Supported | A call that matched torrents and changed none of them answers `Fails.`, not `Ok.` — see below |
| `torrents/resume`, `start` | Supported | Same |
| `torrents/categories` | Supported | The configured categories |

**Why a pause or resume that changed nothing is a failure.** `apply_download_action` reports a
per-file refusal inside its answer rather than as an error, so a run in which nothing at all
succeeded still comes back `Ok`. Sonarr and Radarr mark an item handled on any 2xx and never
return to it, so an `Ok.` after a pause that did nothing loses the item. When the action matched
torrents and affected none of them, the adapter answers `Fails.` and logs the reasons
(`crates/rd-api/src/compat/qbittorrent/torrents.rs`, `act`). A partly applied run still answers
`Ok.`: the protocol has no shape for "three of five", and a retry must not undo the three that
worked.
| `torrents/createCategory`, `setCategory` | Accepted, no effect | Categories are configured in rDownloader; routing rules decide the destination |
| `torrents/setShareLimits` | Supported | Mapped onto the per-torrent seeding override; `-1` means "use the global setting", `-2` means "no limit" |
| Everything else | `404` | |

Torrents are addressed by their real BitTorrent info hash, read from the stored metadata. A
magnet whose metadata has not resolved yet is listed under the hash from its `xt=urn:btih:`
parameter, so a torrent is findable the moment it is added rather than minutes later. Only
the 40-character v1 hex form is used; a base32 or v2 `btmh` magnet is not listed until real
metadata arrives, because a converted-by-guesswork hash would never match what the client is
waiting for.

## Deliberate departures

- **`addurl` and non-magnet `urls` are refused.** Fetching a URL chosen by the caller would
  turn an API key into a way to reach whatever the machine running the service can reach.
  Clients fall back to uploading the file, which all of them support.
- **`del_files=1` / `deleteFiles=true` do not delete finished files.** The native removal
  path already cancels transfers and clears part files. Deleting completed data that a client
  may have just imported stays a deliberate action inside the application.
- **Categories are not created by a client.** `createCategory` and `setCategory` acknowledge
  without changing anything: clients create their category on every connect and treat a
  refusal as a broken server, while routing rules decide where files land.
- **Neither adapter appears in `web/openapi.json`.** The contracts belong to SABnzbd and
  qBittorrent. Publishing them as ours would make them promises we cannot keep on behalf of
  someone else's client.

## Application matrix

| Application | Usenet (SABnzbd) | Torrents (qBittorrent) |
| --- | --- | --- |
| Sonarr | Contract-verified | Contract-verified |
| Radarr | Contract-verified | Contract-verified |
| Lidarr | Contract-verified | Contract-verified |
| Readarr | Contract-verified | Contract-verified |
| Prowlarr | Not applicable | Not applicable |

**"Contract-verified" is not "tested against that application."** These four use the same
download-client implementations, and `crates/rd-api/tests/compat_arr.rs` replays the call
sequence they issue — test connection, read configuration, add a release, poll for it,
inspect it, remove it — asserting on the fields each step depends on. What that proves is
that the contract holds end to end. It does not prove that a particular version of Sonarr is
satisfied, because no such instance runs in this test suite. Verification against real
instances is still open.

Known limits that a real run would meet first:

- Progress reporting is a snapshot: SABnzbd's `timeleft` and `eta` are `0:00:00` and `unknown`,
  qBittorrent's `dlspeed` is zero and its `eta` is 8 640 000 — qBittorrent's own value for
  "infinite" — rather than estimated, because a queue-wide estimate cannot be derived from one
  poll. Clients display these but do not act on them.
- The qBittorrent adapter reports a package's category by name and honours the `category`
  filter, so a tool sees exactly what it queued — provided the category exists here under that
  name (see *Setting it up*). The SABnzbd adapter still reports `cat` as `*`.
- Seed ratio and upload counters are reported as zero on the qBittorrent side outside the
  seeding override; the native API is where torrent statistics live.
