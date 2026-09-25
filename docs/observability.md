# Observability

What the service tells a monitoring system about itself, and what it keeps about its own past.
Four surfaces. Two from RD-110-01: the Prometheus exposition at `GET /api/v1/metrics` and the
persistent transfer statistics behind `GET /api/v1/stats/transfers` and the Statistics page. Two
from RD-110-03: the **audit log** and the optional **trace export**, both below. The log viewer
and the diagnostic bundle are RD-110-02 and live in `docs/diagnostics.md`.

## Scraping

The route answers in the Prometheus text format (`text/plain; version=0.0.4`), or as
`application/openmetrics-text; version=1.0.0` when the `Accept` header asks for it. Both end in
`# EOF`. It is authenticated like every other route — a session, or a bearer — and it costs a
scope of its own, **`api:metrics`**:

- A token holding `api:metrics` reaches `/api/v1/metrics` and **nothing else**: not the queue,
  not `/api/v1/stats/transfers`, not the settings, and not `/mcp`. A scrape target is pasted into
  a configuration file and lives there for years, which is why it does not double as a reader.
- `api:read` does **not** reach the metrics. The only things that do are `api:metrics`, the
  legacy `api:*`, and an interactive session.
- Without a credential the route answers `401` and says nothing — not even a family name.

Create the token under **Settings → API & MCP** and tick **Metrics** alone. A scrape
configuration then looks like this:

```yaml
scrape_configs:
  - job_name: rdownloader
    metrics_path: /api/v1/metrics
    authorization:
      credentials: <the token>
    static_configs:
      - targets: ['127.0.0.1:8710']
```

`tests/scope_matrix.rs` walks the policy table and proves the refusals; `tests/metrics.rs`
proves the island in both directions and that `/mcp` refuses the token as well.

## The families

Every figure is derived at scrape time from the queue, the scheduler and the statistics tables.
Nothing is counted in memory: a restart loses nothing, and the counters read a table the
retention sweep never touches, so they are monotonic for the life of the database.

| Family | Type | Labels | What it says |
| --- | --- | --- | --- |
| `rdownloader_build_info` | gauge | `version` | Always `1`; the version rides in the label. |
| `rdownloader_uptime_seconds` | gauge | — | Seconds since the service started. |
| `rdownloader_queue_downloads` | gauge | `kind`, `state` | Downloads in the queue, per transport and state. |
| `rdownloader_queue_wait_seconds` | histogram | `le` | How long the downloads that are still `queued` have been waiting. Buckets: 10 s, 1, 5, 15 min, 1, 4 h, 1 d, 7 d. |
| `rdownloader_runners_active` | gauge | `kind` | Downloads a runner is working on (resolving, downloading, seeding). |
| `rdownloader_transfer_rate_bytes_per_second` | gauge | `kind` | The current smoothed rate, the same figure the interface shows. |
| `rdownloader_transfers_total` | counter | `kind`, `provider`, `outcome` | Transfers that ended: `completed` or `failed` (final failure or blocked). |
| `rdownloader_transfer_bytes_total` | counter | `kind`, `provider` | Bytes of completed transfers. |
| `rdownloader_transfer_retries_total` | counter | `kind`, `provider` | Attempts that failed and were scheduled again. |
| `rdownloader_transfer_seconds_total` | counter | `kind`, `provider` | Seconds from creation to completion, summed; divide by `transfers_total{outcome="completed"}` for the mean turnaround. |
| `rdownloader_provider_accounts` | gauge | `provider`, `enabled` | Configured hoster accounts. |
| `rdownloader_provider_hosts_blocked` | gauge | — | Hosts the scheduler is holding back after a rate limit or an address block. |
| `rdownloader_storage_free_bytes` | gauge | `target` | Free space of each storage destination. |
| `rdownloader_storage_total_bytes` | gauge | `target` | Size of the volume behind it. |
| `rdownloader_storage_blocked` | gauge | `target` | `1` while intake to the destination is held back for lack of space. |

### Why the label set is closed

Every label value comes from a closed set: `kind` is the transport enum (`http`, `usenet`,
`media`, `gallery`, `record`, `torrent`, `ftp`, `sftp`, `plugin`), `state` the download state
enum, `outcome` one of two words, `provider` the provider id of the account a transfer used (or
`direct`), bounded by the installed plugins, and `target` a storage-root id or `fallback`. Never a
URL, a file or package name, an account label, a user name or a host. Two things enforce it:

- The formatter (`crates/rd-api/src/metrics_format.rs`) is fed by one collector
  (`crates/rd-api/src/metrics.rs`) that only ever hands it enum names and ids.
- `tests/metrics.rs` queues three hundred downloads, each with its own address and file name,
  under a package with a private name and beside an account with a label and a user name, and
  asserts that the number of series does not move and that none of those strings is in the text.

There is no metrics crate. The exposition format is comment lines, a name, labels with three
escaped characters and a value; the formatter is about a hundred lines with a golden test
against the format, and a registry with a lifetime of its own would have nothing to hold.

## Persistent transfer statistics

Migration `0077` adds two tables with the same five figures — `completed`, `failed`,
`retries`, `bytes`, `seconds` — kept for two different questions:

- **`transfer_stats`** answers *what happened when*: one row per hour, transport and provider,
  later folded into one row per day. `bucket_start` is fixed-width UTC RFC 3339, so ordering and
  range comparison are string comparison.
- **`transfer_totals`** answers *how much altogether*: one row per transport and provider, only
  ever added to and never pruned. The counters above read it.

Both are written **inside the transaction** that completes or fails a download
(`crates/rd-db/src/writer_jobs.rs`), so a figure never describes a transfer the queue does not
know about, and a crash between the two cannot leave them apart. `seconds` is the time from the
download's creation to its completion — the whole turnaround, not the transfer alone, because a
start time is not persisted and a figure that would be guessed is worse than a wider one that is
named.

### Retention and downsampling

Two settings under **Settings → System**:

- **Keep hourly figures** (`stats_hourly_days`, default 30, 1 to the retention): hourly rows older
  than this are added into their day's row.
- **Retention** (`stats_retention_days`, default 365, 7–3650): rows older than this are deleted.

An out-of-range pair is refused at save time with `settings.stats_retention_invalid`.

The sweep (`crates/rd-api/src/stats_retention_service.rs`) runs an hour after start and hourly
after that. It goes through the serialized writer like every other mutation, but in **batches of
at most 500 rows** (`rd_db::PRUNE_BATCH`) with a pause between two batches, so the longest a queue
write can wait behind it is one batch. `crates/rd-db/src/stats_tests.rs` seeds three thousand
stale rows, sweeps them while creating a download, and asserts that the download is written
before the sweep is over and that no batch exceeded its bound. The settings are re-read on every
pass, so a change applies without a restart.

### The REST surface

`GET /api/v1/stats/transfers?range=day|week|month|year` (scope `api:read`) returns the folded
buckets — hourly for `day`, daily otherwise — the range summed by kind and by provider, the range
total, and the all-time total. Both bucket widths are read and folded to the requested one, so a
short hourly window does not lose the hours that have already become days.

## The audit log

A separate, append-only record of the security-relevant things that happened: who acted, from
where, on what, and how it ended. Read it at **Audit log** in the sidebar, or through
`GET /api/v1/audit/records`.

### Why it is not the log store

A log record may be dropped — the capture layer hands records to a bounded channel with
`try_send` and counts what it could not fit, because a download engine must not stall behind the
database. An audit record may not: it goes through the serialized writer and the action that
caused it **waits for it**, so no action is reported as done without its record. The two also
differ by an order of magnitude in how long they are kept, and an audit record is a closed
vocabulary a filter can be written against rather than free text. Hence its own table
(`audit_records`, migration `0079`) and its own page.

### What is recorded

| Action | When |
| --- | --- |
| `login_succeeded`, `login_failed`, `logout` | Every sign-in, accepted or refused, by password, password with a code, or a passkey; and every deliberate sign-out. |
| `password_changed` | The administrator password was replaced by somebody who knew the current one, or an attempt to was refused. The record names the **stage** that refused (`current_password`, `locked_out`) and how many sessions the change ended -- never either password. |
| `token_used` | An API or capture token was accepted on a request. At most once per token per hour, because a scrape target would otherwise bury everything else. |
| `token_created`, `token_rescoped`, `token_revoked` | A machine credential was minted, widened or narrowed, or withdrawn. |
| `settings_changed`, `settings_reset` | The settings document was written or reset. The record names the **fields** that changed, never their values. |
| `plugin_installed`, `plugin_removed`, `plugin_key_revoked`, `plugin_digest_revoked`, `plugin_digest_unrevoked` | Every trust decision about code this service runs. |
| `download_deleted`, `package_deleted`, `category_deleted`, `storage_root_deleted`, `backup_restored` | The destructive actions. |
| `logs_cleared`, `audit_cleared`, `stats_cleared` | A store was emptied from the settings (RD-120-34). `audit_cleared` is the one record the emptied audit log then holds: it is written in the same transaction as the delete, so the log is never empty with nothing saying why. All three carry `removed_records`. |
| `notifications_cleared` | The notification history was emptied from its list (RD-130-08). Deliveries still queued or retrying stayed; `removed_records` counts only what went. |

Every record carries the action, the outcome (`success` or `failure`), the actor kind
(`session`, `token`, `anonymous` or `system`) with an opaque id and, for a token, its label, the
client address where a request carried one, the target as a family and an id with the name a
person gave it, the trace, and a flat map of details.

### What is never recorded

No password, no token, no token digest, no signed URL. A refused sign-in names the **stage** that
refused — `password`, `password+totp`, `locked_out` — and never the attempt, because a log of
near-miss passwords is the one thing this must not become. A settings change names field names.
A token is its id and its label. Every string passes `rd_core::redact_text` on the way in and a
detail whose key reads like a credential is replaced whole, exactly as the log store's records
are; `crates/rd-api/tests/audit.rs` searches whole pages for the canaries.

### Append-only

Three places at once, so no single mistake removes the property:

1. There is no writer command that updates an audit row.
2. There is no route that writes or edits one, and none that deletes a *chosen* one. The
   documented surface of the area is two `GET`s and one `POST` that empties the whole log
   (RD-120-34) — a clear that cannot pick its rows is a fresh start, not a way to rewrite
   history.
3. Migration `0079` carries a trigger that aborts **every** `UPDATE` on the table with
   `RAISE(ABORT)`, including one issued by something outside this workspace.

Deleting a download, a package, a category or a storage root leaves its record behind, and
`backup_store::replace_all` does not list the table, so restoring a configuration backup cannot
erase the record of itself. What this is **not** is a protection against somebody with write
access to the database file; it is a record of what happened, and the job says so.

Two things remove a record and neither can choose which. The first is
`POST /api/v1/audit/records/clear` (scope **`api:admin`**, `confirmed: true` in the body or it is
refused as `data_reset.not_confirmed`), which empties the log on request and **writes itself into
the emptied log as its first entry** — the delete and that entry commit in one transaction, so
there is no moment at which the log is empty and nothing says why, and `removed_records` in its
details says how many went. That self-entry is the condition on which the action exists at all.

The second is retention, and it removes whole rows, oldest first:
`audit_retention_records` (default 100000, 10000–2000000) and `audit_retention_days` (default
365, 30–3650), under **Settings → System**, refused out of range as
`settings.audit_retention_invalid`. The sweep runs beside the log sweep in
`rd_diagnostics::sink`, in batches of 500 with a yield between them.

### Reading it

`GET /api/v1/audit/records` (scope **`api:admin`**) filters by `action`, `outcome`, `actor_kind`,
`actor_id`, `target_kind`, `target_id`, `trace_id`, `since`, `until` and `before_id`, up to 500
rows a page, newest first; an unknown filter word is refused as `audit.invalid_query` rather than
quietly matching everything. `GET /api/v1/audit/export` returns the same filter as
newline-delimited JSON, up to 10000 rows, as a file. Both cost `api:admin` and not `api:read`: an
audit log names who acted, from which address, on what, and a read token pasted into a status
page must not reach it.

## Trace context and the optional OTLP export

### The trace

Every request takes the caller's `traceparent` header when it sends a readable one and starts a
root of its own otherwise, and the response carries it back. The id is a span field, `trace_id`,
and `rd_diagnostics::capture` copies it onto every log record written inside that span — so one
id ties the request, everything it logged and the audit record of what it did.

Queued work does **not** inherit the request's trace, on purpose: the scheduler picks a download
up minutes later, in another task, possibly after a restart. It derives its trace from the job
instead (`rd_core::TraceContext::for_job`), so every span about download `42` — the scheduler's
attempt, the resolver call inside it — and the post-processing of its package share one id
across restarts with nothing stored or passed. The cost is that the HTTP request which created
the download and the later transfer are two traces, joined by the audit record and the download
id rather than by the trace id.

Paste a trace id into the log viewer's **Correlation id** filter or the audit log's **Trace id**
filter to read one piece of work end to end.

### Exporting

Off by default, and off after an upgrade: exporting traces sends the shape of a person's
activity to a third system. Switch it on under **Settings → System**:

- **Export traces over OTLP** (`otlp_enabled`, default off).
- **Collector endpoint** (`otlp_endpoint`): an OTLP/HTTP traces URL, `http` or `https` only, for
  example `http://127.0.0.1:4318/v1/traces`. Switching the export on with no endpoint is refused
  as `settings.otlp_endpoint_required`; anything that is not an absolute `http`/`https` URL is
  refused as `settings.otlp_invalid`.
- **Export timeout** (`otlp_timeout_seconds`, default 5, 1–60).

Both the endpoint and the switch are **privileged** settings: they tell the service to post to a
URL of somebody's choosing on a timer, so an `api:config` token cannot change them. A refused
attempt is itself audited.

Spans are posted in batches of at most 512, every two seconds, as the proto3 JSON mapping of
`ExportTraceServiceRequest` — the encoding OTLP specifies for `Content-Type: application/json`.
Only spans that belong to a trace are built at all: one carrying a `trace_id` field, and anything
opened inside one. While the export is off the layer costs one atomic load per closed span.

### It cannot cost you a download

- Spans go into a bounded channel with `try_send`; a full channel drops and counts.
- A failed export drops its batch and is **never retried**. A retry queue would grow in memory
  for exactly as long as the outage lasts.
- Unreadable settings switch the export off rather than sending to a stale endpoint.
- `crates/rd-api/tests/audit.rs::an_unreachable_collector_does_not_block_a_request` points the
  export at a closed port and runs ten requests: all answered, all audited, well inside the
  timeout.

### Nothing sensitive leaves

Every span attribute passes `rd_diagnostics::capture::redact_field`, the same function the log
records pass, before it is stored or encoded. The request span carries the HTTP method and the
**matched route pattern** (`/api/v1/downloads/{id}`), never the raw URI — a path parameter is a
download id at worst, a raw URI is where a signed link would be.
`crates/rd-diagnostics/tests/otlp.rs::no_span_attribute_carries_a_known_secret_pattern` checks
the encoded document itself against the canaries.
