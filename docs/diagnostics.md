# Logs and diagnostics

What the service keeps about its own behaviour, how to read it, and how to hand somebody else
enough to diagnose a problem without handing them a credential. Two things live here, both from
RD-110-02: the **structured log store** behind *Logs* in the sidebar, and the **diagnostic
bundle** at the bottom of that page.

## The log store

Every `tracing` event the service lets through its log filter is also written to the database,
in the table `log_records`. A record carries:

| Column | What it is |
| --- | --- |
| `recorded_at` | UTC, milliseconds |
| `level` | `debug`, `info`, `warn` or `error` — `trace` is never stored |
| `component` | the `tracing` target, such as `rd_http::engine` |
| `code` | the event's stable `code` field, when it carried one |
| `correlation_id` | the first of `correlation_id`, `trace_id`, `request_id`, `download_id`, `package_id`, `job_id` found on the event or on an enclosing span, innermost span first. `trace_id` is what RD-110-03 puts on a request and on a queued job, so pasting a trace id from the audit log into this filter reads one piece of work end to end; see [`observability.md`](observability.md) |
| `message` | the event's message |
| `fields` | every other field, rendered as text |

The store sees what stderr sees: the same `RUST_LOG` filter governs both, so `RUST_LOG=debug`
fills the store with debug lines and the default (`rdownloader=info,rd_=info`) keeps it to
`info` and above.

### Redaction happens before storage

The capture layer runs every message and every field value through `rd_core::redact_text`
before the record leaves the layer, and a field whose *name* says it holds a credential —
`password`, `token`, `api_key`, `authorization`, `cookie`, the signed-URL parameter names — is
replaced as a whole by `[redacted]`. The store, the REST API and the viewer only ever carry what
the layer produced; nothing downstream re-reads the log file. The secret canaries in
`crates/rd-diagnostics/tests/redaction.rs` and `crates/rd-api/tests/diagnostics.rs` hold that
line: tokens in query strings, passwords in `ftp://user:password@` addresses, AWS, Azure and
Google signed-URL parameters, `Authorization` and `Cookie` values, bearer schemes and `vault://`
references are logged and then shown to be absent from the record, the database and the API
response.

Parameter *names* survive — `X-Amz-Signature=%5Bredacted%5D` inside a URL, since the value is
written back through the URL serializer, and `[redacted]` everywhere else — because a support
log that has lost the name is harder to read than one that kept it with a placeholder value.
Userinfo in an address (`ftp://user:password@host/`) is replaced as a whole by `redacted@`.

### Logging never blocks

The layer hands each record to a bounded channel (4096 records) with a non-blocking send. When
the sink is behind, the record is dropped and counted, and the thread that logged goes on; the
viewer shows the drop count above the list. A download engine does not stall because the
database is busy writing its own log.

The sink writes in batches — at most 200 records per writer command, at least every 250 ms — so
a chatty minute costs the serialized writer a handful of transactions rather than a thousand.

### Retention

Two settings under **Settings → System → Log retention**, saved with the settings document:

| Setting | Default | Range |
| --- | --- | --- |
| Records kept (`log_retention_records`) | 20 000 | 1 000 – 500 000 |
| Days kept (`log_retention_days`) | 14 | 1 – 365 |

The sweep runs once a minute and after every 2 000 new records, reads the settings fresh each
time, and deletes in steps of at most 2 000 rows with a yield between them. The writer serves
queue mutations in the gaps, which is what "deletion never blocks the queue" means in practice.
A lowered setting takes effect at the next sweep, without a restart.

## The viewer

*Logs* in the sidebar. The filter bar sits above the list: minimum level, component prefix
(`rd_http` matches `rd_http::engine`), exact code, exact correlation id, and a case-insensitive
search in messages. Each row shows time, level, component, code, correlation id and message;
a record with further fields opens them behind the chevron. The list says when it is a full
page and offers the page behind it, and it states the retention in force.

## The REST API

All four operations cost the `api:admin` scope: diagnostics are the service itself, and a
read-only token pasted into a status page must not become a way to read the log.

| Operation | What it does |
| --- | --- |
| `GET /api/v1/diagnostics/logs` | The newest records, newest first. Query: `level`, `component`, `code`, `correlation_id`, `search`, `since`, `until` (RFC 3339), `before_id`, `limit` (1–500, default 200). The answer carries `full_page`, `total`, `captured`, `dropped` and the retention. |
| `GET /api/v1/diagnostics/bundle/preview` | The inventory a bundle would hold, its `digest`, what is never included, and the directory it would be written to. |
| `POST /api/v1/diagnostics/bundle` | Writes the bundle. Body: `{ "approved": true, "digest": "<from the preview>", "entries": ["versions", …] }`. |
| `GET /api/v1/diagnostics/bundles/{name}` | Downloads a bundle written earlier; only a name the service generated is opened. |

Refusals carry stable codes: `diagnostics.invalid_query` (400), `diagnostics.approval_required`
(400), `diagnostics.preview_stale` (409), `diagnostics.bundle_not_found` (404),
`diagnostics.no_data_directory` (409); the retention range is `settings.log_retention_invalid`.

## The diagnostic bundle

An archive for a support request, written **on this machine only** — there is no upload, and
none is planned — under `<data directory>/diagnostics/rdownloader-diagnostics-<UTC>.zip`.

It is created in two steps, and the second cannot skip the first. **Preview contents** shows the
inventory: every file the archive would hold, what it contains, how many items are in it right
now, what was redacted in it, and what no bundle ever contains. Untick what the recipient should
not get. **Create bundle** sends the approval together with the preview's digest; the server
recomputes the inventory and refuses with `diagnostics.preview_stale` if it no longer matches,
so a client that never drew the preview cannot know the digest and an inventory that changed
under the approval has to be looked at again. The digest covers what the entries *are* — id,
path, kind — not how many items they hold, so a warning logged between preview and approval
does not turn the approval stale.

| File | Contents |
| --- | --- |
| `manifest.json` | Format version, creation time, application version, the inventory digest, every file with its English description, size and SHA-256, the entries that were unticked, the redactions applied, and what is excluded by rule |
| `versions.json` | Application version, OS and architecture, installed plugins with their versions |
| `configuration.json` | The settings document with every credential-named field replaced by `[redacted]`, PEM material replaced by `[present]`, and every string passed through the URL and header redaction |
| `system-checks.json` | Tool locations, versions and compatibility verdicts, the reverse-proxy contract — as structured checks |
| `doctor.txt` | The same checks rendered exactly as `rdownloader doctor` prints them |
| `recent-errors.json` | The newest 200 records at `warn` and above, and how many records the store holds |

**Never included, whatever the settings hold:** file contents and file names of downloads, full
URLs with credential-bearing query values, and secrets of any kind — passwords, tokens, API
keys, cookies, certificates, vault references.

The archive is a function of the state and the clock: the same inventory, the same selection and
the same creation time produce byte-identical archives (fixed entry timestamps, sorted
structures), which is what makes a manifest comparable across two bundles from the same
installation. `crates/rd-diagnostics/tests/bundle.rs` holds both properties.

### The preview speaks your language, the archive speaks English

The page that asks *what may the recipient see* has to be readable by the person answering it,
so the server writes no sentences (RD-120-15). Every line of the inventory — what a file holds,
what was redacted in it, what no bundle ever contains — travels as a stable code
(`diagnostics.bundle.entry.configuration.description`, `diagnostics.bundle.redaction.paths`, …)
together with the data the sentence names, such as the list of field names that were actually
replaced. Those field names are data and stay as they are in every language. The interface looks
the code up under `logs.<code>` in `web/src/locales/{de,en,es,fr}/logs.json` and shows the
sentence in the reader's language; a code no catalogue knows yet shows the English that came
with it, never the raw code.

The archive itself is **not** translated, and deliberately so: `manifest.json` is read in a
support queue by somebody who does not have this installation in front of them, so every entry
carries a plain English description and the redaction and exclusion lines are English sentences.
`crates/rd-diagnostics` does not hold those sentences either — it reads them at compile time out
of `web/src/locales/en/logs.json`, the same catalogue the interface translates from, so the two
readers of one inventory cannot drift apart.

The inventory digest covers what an entry *is* and never what it says, so two people reading in
two languages approve the same bundle from the same state.
`crates/rd-diagnostics/tests/notes.rs` holds all of it.

### `doctor` and the bundle agree

`rdownloader doctor` prints its tool and reverse-proxy sections from
`rd_api::diagnostics_checks::system_checks`, the same function the bundle calls. The toolchain
lines (cross-build helpers) stay in the command; they are about building the software, not
running it.
