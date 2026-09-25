# Site rules

A release page is recognised because a **rule** describes it, not because somebody built a
plugin for it. A rule is data: the addresses a service uses, the steps that turn one of its
pages into a list of links, and where the package name comes from. Since 1.3 (RD-130-07) no
rule arrives with the program: the project's rules are one signed file,
`rdownloader-site-rules.json`, that every release carries beside its packages, and importing it
under *Settings → Site rules* verifies it and stores its rules in the database as the person's
own, switched off. From then on they are ordinary rules — edited, duplicated, switched and
removed like any the person wrote.

This page is both halves: the format (RD-110-04) and what each step *does* when it runs
(RD-110-05, "The executor" below). The crate is `crates/rd-siterules`, a leaf: `rd-sign`,
`serde`, `regex`, `url`, `base64`, `hex`, `percent-encoding` and a clock — no HTTP client, no
captcha broker, no database. The executor takes those as traits from its caller, which is
what lets every limit below be proven without a network.

## Trust

The pack is a `SignedDocument` (`crates/rd-sign/src/envelope.rs`) under its own role,
`Role::SiteRules`, with its own compiled-in root key `rdownloader-siterules-v1` and the
domain string `rdownloader.site-rules.v1`. The domain is bound into the digest, so a
signature over a tool manifest or a plugin repository index does not verify as a rule pack
and a rule-pack signature does not verify as anything else — even under the same key. The key
is separate from the plugin and tool-manifest keys because a rule pack is edited far more
often than either, and one compromise must not vouch for everything.

Revocation and freshness are the ones every signed document in this project already has:
a pack whose digest is in the trust store's revocation set is refused, a pack whose signing
key was revoked is refused, and `rd_sign::replay` refuses a sequence that is not newer than
the one already accepted, an expired `not_after`, and an `issued_at` further in the future
than clock skew explains. The import checks the file with no known sequence: it turns the pack
into rules of the person's own rather than keeping the pack, so there is no earlier sequence for
a later file to be measured against, and a second import meets the rules the first stored and
refuses each as `site_rules.duplicate_id` rather than writing over an edit. A file that fails
any check is refused whole with the pack's own code — `site_rules.malformed`, `.untrusted`,
`.bad_signature`, `.revoked`, `.format_version_unsupported`, `.stale` or `.invalid_rule` — and
never read a second time as an unsigned export.

## The pack

```json
{
  "format_version": 1,
  "sequence": 12,
  "issued_at": "2026-09-20T12:00:00Z",
  "not_after": "2027-03-20T12:00:00Z",
  "rules": [ ... ]
}
```

| Field | Meaning |
| --- | --- |
| `format_version` | The layout this build understands: `1`. A pack declaring any other number is refused as a whole with `site_rules.format_version_unsupported`; no rule from it is read. |
| `sequence` | The publisher's monotonic counter. Never goes backwards. |
| `issued_at` | When the publisher signed it (RFC 3339). |
| `not_after` | Optional. After this instant the pack is stale even if nothing newer has been seen. |
| `rules` | The rules. Every `id` is unique; a repeat refuses the pack. |

The checks run in this order, and the first failure refuses the whole pack: envelope,
revocation and signature, format version, freshness, every rule, duplicate ids. The format
version is checked *after* the signature on purpose — an unsigned document that declares a
future version is still an unsigned document.

Unknown fields are an error everywhere in the format, not ignored: a field this build does
not know is a format it does not understand. There is no installed base that would need a
softer reading.

| Code | When |
| --- | --- |
| `site_rules.malformed` | Not JSON, not the envelope, a missing `format_version`, an unknown field, a wrong type. |
| `site_rules.untrusted` | No signature from a key this installation trusts. |
| `site_rules.bad_signature` | A trusted key was named and the signature did not hold: tampering, corruption or a foreign domain. |
| `site_rules.revoked` | The pack's digest was withdrawn. |
| `site_rules.format_version_unsupported` | `format_version` is not `1`. |
| `site_rules.stale` | Replayed, expired, or issued too far in the future. |
| `site_rules.invalid_rule` | One rule failed validation; the error names its `id` and the reason. |
| `site_rules.duplicate_id` | Two rules in the pack share an `id`. |

## A rule, field by field

The example is the one `crates/rd-siterules/src/format_tests.rs` keeps in step with this
page: the test parses exactly this, validates it and round-trips it.

```jsonc
{
  // Stable identifier: lowercase kebab-case, at most 64 characters, unique within the
  // installation; a second rule under a taken id is refused (see "User rules").
  "id": "scnlog",
  // What the interface shows. Up to 120 characters.
  "name": "scnlog.me",
  // What the interface groups and switches by: `board`, `paste`, `adult`, ... Lowercase
  // kebab-case, at most 32 characters; the list is open.
  "group": "board",
  // The rule's own revision, from 1. Bumped when the rule changes, so a copy of a rule
  // can say which revision it started from.
  "version": 1,
  // The addresses this rule claims.
  "match": {
    // Concrete hosts, or `*.example.org` for a host and everything below it (the apex
    // itself included). The first entry is the canonical host and must be concrete.
    "hosts": ["scnlog.me", "*.scnlog.me"],
    // Regular expressions over the path, with the query string appended when there is
    // one. Empty claims every path on those hosts.
    "paths": ["^/[a-z0-9-]+/[a-z0-9-]+/?$"]
  },
  // Hosts the service once had and that no longer answer. An address on one of them is
  // rewritten to the canonical host instead of being refused, so a bookmark from a year
  // ago still works. A dead host may not also be claimed by `match`.
  "dead": ["scnlog.eu", "scnlog.life"],
  // What to do, in order. At least one step.
  "steps": [
    { "kind": "fetch" },
    { "kind": "regex", "pattern": "<div class=\"links\">(.+?)</div>", "into": "container" },
    { "kind": "regex", "from": "container", "pattern": "href=\"(https?://[^\"]+)\"",
      "into": "links", "all": true }
  ],
  // Where the package name comes from.
  "package": { "from": "regex", "pattern": "<h1[^>]*>(.+?)</h1>" },
  // Optional, default false. True when the links behind one address of this rule are copies
  // of the same file rather than different files -- the shape a release page has. See
  // "One release, many mirrors" below.
  "mirrors": true,
  // A real address the self-test (RD-110-09) fetches. Absolute `http` or `https`, and one
  // this rule's own `match` claims.
  "probe": "https://scnlog.me/movies/some-release-2026/",
  // The date the service was last measured alive.
  "checked": "2026-09-20"
}
```

### Hosts

A host is written the way the address bar hands it over: lowercase, at least two DNS
labels, no scheme, port or path; an internationalised name in its punycode form.
`*.example.org` claims `example.org` and every name below it. `localhost`, an IP address
and a single label are not hosts here — a rule describes a public service.

### Variables and templates

A step reads from and writes to named variables (`[a-z][a-z0-9_]*`, at most 32
characters). `fetch` writes the page into `page` unless `into` says otherwise, `regex`
reads from `page` unless `from` says otherwise, and the link list a rule produces is the
variable `links` when the last step has run. A field marked *template* may embed `${name}`;
a placeholder that does not close, or names something that is not a variable, refuses the
rule.

### Steps

The seven kinds, by shape. What each one *does* when it runs is "The executor" below. The
validation refuses what cannot work without touching the network — a pattern that does not
compile, a malformed variable name, an unterminated placeholder.

| `kind` | Fields | Does |
| --- | --- | --- |
| `fetch` | `url` (template, optional: the claimed address), `into` (optional: `page`) | Fetches a page as text. |
| `fetch-json` | `url` (template), `path` (JSON pointer, RFC 6901, starts with `/`), `into` | Fetches JSON and takes the value at `path`. |
| `regex` | `pattern`, `from` (optional: `page`), `into`, `all` (optional, default `false`) | Applies the pattern; the first capture group is the value. With `all`, every match is taken and the variable becomes a list. |
| `decode` | `encoding` (`base64`, `hex`, `rot13`, `url`, `js-string`), `from`, `into` | Decodes a variable. `js-string` is concatenated JavaScript string literals, `"a" + 'b'`. |
| `form` | `url` (template), `fields` (map of name to template, optional), `into` (optional) | Submits a form and keeps the response. |
| `redirect` | `from`, `into` | Follows the redirect an address answers with and keeps the target. |
| `captcha` | `challenge` (kind, e.g. `recaptcha-v2`), `sitekey` (template, optional), `into` (optional) | Hands a challenge to the captcha broker. The kinds the broker knows are RD-110-15's. |

### Package source

| `from` | Fields | Meaning |
| --- | --- | --- |
| `title` | — | The page's `<title>`. |
| `regex` | `pattern`, `source` (optional: `page`) | The first capture of `pattern` applied to a variable. |
| `variable` | `name` | A variable a step wrote. |

### One release, many mirrors

`"mirrors": true` says that the links behind one address of this rule are the same file at
several hosters, not several files. It is what makes the LinkGrabber group them (RD-110-18):
every link the run produced arrives carrying one mirror key, so five hosters become one entry
with five mirrors instead of five candidates of which four get deleted by hand. A mirror is
kept, not discarded -- it is what remains when the chosen one goes offline.

It is a statement about the **page**, not about a link, because the rule format carries no
per-link metadata. A page that lists several different files, each with its own mirrors,
cannot be described this way and leaves the field out: a wrong group is worse than none. The
key the host builds from it names the rule and the crawled address, so two such pages read
into one package stay two groups.

The field names no quality and no language for the same reason. Where a release name spells
them out, `rd_collector` reads them off it; where it does not, the group carries neither.

Absent from a rule that does not set it, and absent from the serialized form when false, so a
pack signed before this existed keeps its signature.

### What refuses a rule

Every refusal is a `RuleError` with a message that names the field; inside a pack it
surfaces as `site_rules.invalid_rule` with the rule's `id`. The cases: an `id`, `group` or
variable name that is not well-formed, an empty or overlong `name`, `version` `0`, no hosts,
a wildcard as the first host, something that is not a host, a dead host that `match` also
claims, a pattern that does not compile (in `match.paths`, a step or the package source),
no steps, a malformed template, a JSON pointer without its leading slash, a `probe` that is
not an absolute `http(s)` address or that the rule's own `match` does not claim, and a
`checked` that is not a calendar date.

## The executor

RD-110-05. `rd_siterules::Executor` takes a rule and an address and answers with the links
behind that page, the package name and the number of requests it took — or with one stable
code saying why not. It runs **natively**, not as a Wasm guest: a rule is a record, not
somebody else's code, and the sandbox exists to contain somebody else's code. Running it
natively puts the three bolts in one place instead of in every plugin, and a rule edited in
the interface takes effect at once instead of after a signed rebuild. Protectors keep their
own logic and stay Wasm (RD-110-16, RD-110-17).

### What the caller supplies

The crate owns no HTTP client, no captcha broker and no clock. It takes four traits
(`rd_siterules::exec::ports`), and the adapters live with the caller, where the proxies, the
TLS roots and the captcha broker already are — since RD-110-06 in
`crates/rd-plugin-host/src/siterules.rs`; see "Where the adapters live" below:

| Port | Contract |
| --- | --- |
| `Fetcher` | One request: method, form body, a byte ceiling, a timeout **and the addresses the executor already checked** in; status, headers and body out. Two obligations, both load-bearing: it **must not follow redirects** — it reports them, and the executor follows them itself so that every hop is checked again — and it **must connect to `FetchRequest::addresses` rather than resolving the host again** (see "Resolve once, connect to that" below). |
| `HostResolver` | A name to its addresses, called once per request. The ban on private ranges is applied to what this returns. |
| `CaptchaSolver` | Optional. Without one, a `captcha` step refuses with `site_rules.captcha_failed` rather than being skipped. |
| `Clock` | Monotonic time for the run's total budget. A port so the budget has a test that does not spend it. |

### The run

1. An address on one of the rule's `dead` hosts is rewritten to the canonical host.
2. An address the rule's `match` does not claim is refused with `site_rules.not_claimed` —
   the one refusal that means "not my page" and lets the selection (RD-110-06) keep looking.
3. Two variables are seeded: `url`, the address the run was given, and `page_url`, which
   every fetch updates to the address that actually answered.
4. The steps run in order. Each one writes a variable or refuses; there is no half-success.
5. The variable `links` is the result: every entry is resolved against `page_url`, anything
   that is not `http`/`https` is dropped, and so is a link on a literal local address — a
   rule's output must not point the download engine at this machine. What is left is
   deduplicated in order. Nothing left is `site_rules.no_links`, never an empty package.
6. `package` is read last. A package source that finds nothing costs the *name*, not the
   links: the name is a hint, as `crawled-link.package-hint` is.

A variable holds one string or a list of them. `regex` with `all`, `fetch-json` over an array
and `redirect` produce lists; `decode` and `redirect` then work on every element. A list of
one collapses, so `${name}` stays usable after a pattern that happened to match once.

### The steps at run time

```jsonc
// fetch — GET a page. `url` is a template; without it, the address the run was given.
{ "kind": "fetch" }
{ "kind": "fetch", "url": "${url}?page=2", "into": "second" }

// fetch-json — GET, parse, take the value at the pointer. A string, number or boolean
// becomes one value; an array of them becomes a list; an object or a nested array refuses.
{ "kind": "fetch-json", "url": "https://serienjunkies.org/api/1", "path": "/data/items",
  "into": "links" }

// regex — the first capture group, or the whole match when the pattern has no group.
// Nothing matched is `site_rules.structure`: the page's structure changed.
{ "kind": "regex", "pattern": "<div class=\"links\">(.+?)</div>", "into": "container" }
{ "kind": "regex", "from": "container", "pattern": "href=\"(https?://[^\"]+)\"",
  "into": "links", "all": true }

// decode — over every element. base64 reads both alphabets, with or without padding.
{ "kind": "decode", "encoding": "base64", "from": "raw", "into": "links" }

// form — POST `application/x-www-form-urlencoded`; every field value is a template.
{ "kind": "form", "url": "https://board.test/go", "fields": { "t": "${token}" },
  "into": "result" }

// redirect — asks each address what it redirects to and keeps the target. The target is
// *not* fetched, so it may be any host; an address that answers 200 instead refuses.
{ "kind": "redirect", "from": "raw", "into": "links" }

// captcha — hands the challenge to the broker; the token lands in `captcha` by default.
{ "kind": "captcha", "challenge": "recaptcha-v2", "sitekey": "${sitekey}", "into": "answer" }
```

### No JavaScript interpreter

`decode` covers base64, hex, rot13, percent-encoding and concatenated JavaScript string
literals (`"aHR0" + 'cHM6'`, escapes included) — between them almost every obfuscation these
pages use, because almost all of it is arithmetic on text rather than a program.
JDownloader's `GenericBase64Decrypter` covers six services on that alone. What is genuinely a
program stays undecoded and the run refuses with `site_rules.decode_failed`. This is a
decision, not a gap: running a stranger's script to find a download link would hand a release
page the power a plugin has, without the sandbox a plugin runs in — and an honest refusal is
what keeps an HTML file out of the queue (RD-110-07).

### No browser, either — and five services this rules out

Measured 2026-09-21 (RD-110-14). `serienfans.org`, `filmfans.org`, `newalbumreleases.net`,
`multipaste.org` and `zpaste.net` all answer `403` with `cf-mitigated: challenge` on **every**
path of their own application: the root, a real content page, the site's own JSON endpoint,
the feed, the sitemap — and `robots.txt` too, so the operator cannot show a non-browser even
its own crawling policy. The body is Cloudflare's managed-challenge interstitial
(`cType: 'managed'`, `Enable JavaScript and cookies to continue`), which is a program to run,
not a document to read. The challenge response sets **no** cookie, so there is nothing a
handed-over browser session could carry, and `FetchRequest` has no field for a cookie or a
header in any case.

**All five are a documented No-Go** (`docs/adr/0012-sites-behind-a-browser-gate.md`). They
carry no rule, they are not in the release file, and a rule for them is not a matter of
finding the right selectors: there is no response to write one against. Passing the gate would
mean running its script or imitating a browser's fingerprint, which is the line ADR 0010 drew
for filecrypt and this record keeps.

What a rule does when it meets such a gate is the honest refusal, not a workaround:
`site_rules.blocked`, which is what RD-110-09 turns into `blockiert`.

### The bolts

- **Host narrowing.** A rule may request only the hosts its `match` names, plus the host of
  the address the run was given. Everything else is `site_rules.target_not_allowed`, and the
  request is never made.
- **Every redirect re-checked.** The executor follows redirects itself, so each hop passes
  the host bolt and the address bolt again. A page that redirects to a foreign host is
  refused at the hop, and the foreign host is never asked.
- **No private, local or link-local address.** Checked against the address DNS returned, not
  against the name: `127.0.0.0/8`, `10/8`, `172.16/12`, `192.168/16`, `169.254/16`,
  `100.64/10`, `0/8`, `192.0.0/24`, `198.18/15`, `240/4`, broadcast and multicast, and for
  IPv6 `::1`, `::`, `fc00::/7`, `fe80::/10`, multicast, `100::/64`, `2001:2::/48`,
  `2001:10::/28`, `2001:20::/28`, `2001:db8::/32` and `3fff::/20`. *One* non-public address
  among several refuses the request, which is the shape a DNS rebinding attack takes.
- **An IPv4 address wearing an IPv6 costume is judged as the IPv4 address it is.** Four forms
  put an IPv4 address where the IPv6 address itself says it is, and all four are decoded and
  judged by the IPv4 rules as well: IPv4-mapped (`::ffff:a.b.c.d`), IPv4-compatible
  (`::a.b.c.d`), 6to4 (`2002::/16`, RFC 3056 — `2002:0a00:0001::` *is* `10.0.0.1`), NAT64
  with the well-known prefix (`64:ff9b::/96`, RFC 6052) and an ISATAP interface identifier
  (`…:0:5efe:a.b.c.d`, RFC 5214), which may sit under any prefix including a global one. Two
  forms are refused as whole blocks instead of decoded, because decoding them would judge the
  wrong machine or would have to guess: Teredo (`2001::/32`), whose embedded address is the
  client's and whose packets go to a relay, and NAT64 with a network-specific prefix
  (`64:ff9b:1::/48`), which RFC 6052 lays out differently for each of six prefix lengths
  without the address saying which. The same check guards the links a rule *produces*, since
  those are fetched by the download engine.
- **Resolve once, connect to that.** The executor resolves the host itself and hands the
  checked addresses to the fetcher in `FetchRequest::addresses`; the adapter must connect to
  one of them and must not resolve the name again. Otherwise the bolt checks one address and
  the connection uses another: a record with a time-to-live of zero is free to answer
  `93.184.216.34` to the executor and `127.0.0.1` to the client a moment later, and the whole
  check would be theatre. With `reqwest` this is `ClientBuilder::resolve_to_addrs`. The list
  is empty exactly when the host is already a literal address, because nothing was resolved.
- **Cycle protection.** Every address requested in a run is remembered; coming back to one is
  `site_rules.cycle`, so a redirect loop ends at the second hop instead of spending the time
  budget.

### The limits

`rd_siterules::Limits`, each with its own code so a log says which budget ran out:

| Limit | Default | Code |
| --- | --- | --- |
| `max_depth` — how many requests deep an address may sit from the one the run was given; redirect hops count | 6 | `site_rules.limit_depth` |
| `max_pages` — requests in the whole run | 24 | `site_rules.limit_pages` |
| `max_links` — links the rule may produce; refused, not trimmed | 1000, the same as the plugin host's `MAX_CRAWLED_LINKS` | `site_rules.limit_links` |
| `max_total_time` — however few steps there are | 90 s | `site_rules.limit_time` |
| `max_response_bytes` — one body; passed to the adapter *and* checked on what came back | 4 MiB | `site_rules.response_too_large` |
| `request_timeout` — one request, passed to the adapter | 20 s | — |

### Why a run refuses

| Code | When | Keeps the search going |
| --- | --- | --- |
| `site_rules.not_claimed` | The rule's `match` does not claim this address. | yes |
| `site_rules.page_dead` | 404, 410, a name with no address, no route, a refused connection. | no |
| `site_rules.blocked` | 401, 403, 429: the shapes bot protection takes (RD-110-14). | no |
| `site_rules.fetch_failed` | Any other status, a timeout, a broken answer. | no |
| `site_rules.structure` | A pattern, a JSON pointer or a variable found nothing: the page changed. | no |
| `site_rules.decode_failed` | `decode` could not decode, and there is no interpreter. | no |
| `site_rules.captcha_failed` | No broker, a refusing broker, or an empty answer. | no |
| `site_rules.target_not_allowed` | A host outside the rule's `match`, before *and* after a redirect. | no |
| `site_rules.address_not_public` | The target resolves into the local network. | no |
| `site_rules.cycle` | The run came back to an address it had already requested. | no |
| `site_rules.limit_depth`, `.limit_pages`, `.limit_links`, `.limit_time`, `.response_too_large` | A budget ran out. `RunError::is_limit` says so. | no |
| `site_rules.no_links` | Everything worked and nothing was left. | no |

Only `site_rules.not_claimed` sets `RunError::not_mine`. That distinction is the one
RD-110-06 needs: "this was never my page" lets the selection ask the next source, while every
other code is a statement about *this* page and is reported rather than swallowed. RD-110-09
sorts a rule by the same codes — `blocked` into `blockiert`, `page_dead` into `tot`,
`structure` and `no_links` into `strukturell`.

## In the crawler selection

RD-110-06. A rule answers the question a crawler plugin answers — "what lies behind this
address?" — from data rather than from code, so it takes its place in the same selection
rather than beside it. `FolderCrawlers::expand`
(`crates/rd-plugin-ext/src/crawler.rs`) asks three kinds of source in one fixed order:

1. **The crawler plugins that name a service.** A plugin was built for exactly this service
   and has logic of its own.
2. **The rules.** A rule describes a service more broadly than a plugin written for it. Until
   1.2 the person's own came before the shipped ones, so a rule someone wrote could bridge a
   shipped one that had gone wrong; since RD-130-07 every rule is the person's own, the ones
   imported from the release file included, and `Catalogue`'s shipped side stays empty. The
   precedence it still encodes is kept for a pack delivered separately later, and costs
   nothing while that side is empty.
3. **The generic crawlers**, which recognise the *shape* of an address rather than a service
   at all. They are the last resort, and that is what they were already (RD-104-03).

Whoever speaks first answers. Saying "not mine after all" is not speaking: a plugin that
disclaims the address it claimed, and a rule whose `match` does not claim it —
`site_rules.not_claimed`, the one refusal with `RunError::not_mine` — both hand the address
to the next source. Every other code is a statement about *this* page and ends the search
with that code, because handing a dead page to the next source only produces a second
statement about the same page. An address no source claims stays exactly what it was, with
the outcome it had before there were any rules — and since RD-110-07 the online check has the
last word on it: an address that answers with a page rather than a file ends as `unresolvable`
instead of being queued. Because those codes now reach an interface, they are translated: `server.codes.site_rules.*`
in `web/src/locales/{de,en,es,fr}/server.json`: fifteen refusals, the four states of the
self-test below, and `site_rules.probe_invalid`. `not_claimed` is not among
them and must not be — it is never reported, it only moves the search on.

What a rule produced goes through the same acceptance a plugin's answer goes through, in one
place: an address that is not `http`/`https`, one carrying a password, and the very address
the run was given are dropped, and at most `MAX_LINKS` (500) links are taken — the selection's
number, below the executor's own thousand. The package name the rule read travels as the
`package_hint` of every one of those links, the same field a crawler plugin fills, so
`rd_collector::grouping` builds one package named by the page rather than by whichever hoster
delivers the files. There is no second mechanic for rules.

### What a rule's links have to be

RD-110-07. A rule may point anywhere — the file behind a release page is nearly always on
another host, and the WIT contract a crawler plugin answers under says so in as many words.
Nothing checked what came back, and a rule whose container pattern reaches one element too far
returns the board page, the advertisement or the next page of the thread. Such an address used
to become a candidate, and the queue then stored an HTML page under the name of an episode.

Every address either kind of source returns now passes a verdict
(`crates/rd-api/src/collector_crawl_verdict.rs`) before a row exists for it:

1. **A resolver, a transfer backend or a recognised provider claims it** — it goes through
   unchanged, and is deliberately *not* fetched. The resolver turns it into a file later, and
   a `HEAD` against a hoster's landing page would answer `text/html` and prove nothing.
2. **Nobody claims it, and a probe confirms file content** — it goes through as a direct link.
   The judgement is `rd_http::ProbeResult::looks_downloadable`, the same function the transfer
   engine applies mid-download; there is no second HTML test to drift from it. An explicit
   `Content-Disposition: attachment` always wins, which is why a hoster serving the payload
   with `text/html` still counts.
3. **Nobody claims it, and the probe answers with a page** — no candidate. The address is
   counted and refused with the stable code `collector.crawl_not_a_file`.
4. **Nobody claims it, and the probe gets no answer at all** — a timeout, a refused connection,
   a `4xx`. The address is **kept**, unconfirmed, and is not counted as dropped. Silence is not
   proof, and RD-101-06 already settled that a link whose check failed stays queueable rather
   than being treated more harshly than one proven dead; dropping here would let a momentary
   network fault cost a crawler's valid links without saying so. The address is checked like
   any other candidate afterwards, and `looks_downloadable` is applied a second time
   mid-transfer, so a page that slips through still never lands on disk as a file.

The intake's answer carries `crawled_found` and `crawled_dropped`, and the LinkGrabber reports
them together — "12 of 40 found links were not files and were dropped". That is the part a rule
author needs: a pattern that greys one element too many shows up as a number, not as a page that
looks empty.

### Where the adapters live

The executor's four ports are implemented in `crates/rd-plugin-host/src/siterules.rs`, where
the proxy profiles, the custom TLS roots and the captcha broker already are:

| Port | Adapter |
| --- | --- |
| `Fetcher` | `RuleFetcher`, built **per run** by `RuleNetwork::fetcher`. It honours both obligations above: `Policy::none()`, so a redirect is reported and the executor checks the hop, and `ClientBuilder::resolve_to_addrs` on `FetchRequest::addresses`, so the connection goes to the addresses that were checked. It is not taken from `rd_http::ClientPool` for exactly that reason — a client pinned to one host's checked addresses is of no use to any other request, so pooling it would gain nothing and would grow a cache with one entry per host a rule ever visited. One run, one cookie jar, and both die with the run. |
| `HostResolver` | `RuleResolver`, the system resolver. It answers honestly and judges nothing: the ban on private ranges is applied to what it returns. |
| `CaptchaSolver` | `RuleCaptcha` over the application's broker (`rd_captcha::CaptchaBroker` through the `rd_plugin_api::CaptchaSolver` trait). Only the widget kinds cross — `recaptcha-v2`, `hcaptcha`, `turnstile` — because a rule names a kind and a site key and has no picture to hand over; anything else refuses. |
| `Clock` | `rd_siterules::SystemClock`, one per run. |

With a proxy configured the name is resolved at the proxy, so the pinning cannot bind there.
That is the person's own configuration, and the ban on private addresses still refuses a name
that resolves into this network before the request is made.

`rd-plugin-ext` puts the two halves together: `HostRuleRunner` is the executor over those
adapters, and `SiteRules` holds the catalogue and the order. The catalogue is assembled at
start from the switched-on rules in the database
(`crates/rdownloader/src/site_rules_cli.rs`); a rule that does not parse or that the
catalogue refuses costs itself and nothing else. `SiteRules::replace` swaps the rules in force
without a restart, which is what the editor (RD-110-08) writes through.

### A watched page

RD-110-21. A rule reads one release page; a *subscription* on a listing is what turns that
into something that keeps working. The subscription kind **Release page** takes a series,
category or tag address, fetches it conditionally at most every half hour
(`rd_core::SITE_RULE_MIN_POLL_INTERVAL_SECONDS`), and asks `SiteRules::claims` which of the
links on it are release pages — the cheap half of `consult`, same order, same dead-rule skip,
no request. Each surviving link becomes a subscription item whose address enters the
LinkGrabber through the ordinary intake, where the crawler selection then runs the rule
properly and the mirror grouping of RD-110-18 applies. Nothing about mirrors is decided twice.

The adapter is `crates/rd-subscription/src/rule_adapter.rs`; recognition by release name lives
beside it in `release.rs` and reuses `rd_collector::{quality_of, language_of}` rather than a
second token list. What such a subscription is *not* is a second crawler: it reads anchors out
of the listing and judges nothing about them that the rules do not already judge.

## The self-test

RD-110-09. **Rules age, and the measurement that made this a job says how fast.** Of the 124
domains JDownloader carries for this class of page, 47 no longer answered on 20 September 2026,
and four services had vanished outright. That is not a complaint about JDownloader; it is what
these sites do. Without a mechanism the same state walks into rDownloader, and the person meets
it as an empty package rather than as a finding.

**The admission rule, and it is binding.** No rule and no protector plugin is implemented
without the service having been measured first, and the measurement date is written on the job
that implements it. A rule whose service nobody checked is a rule that may already be pointing
at a parked domain on the day it ships. The date travels on in the rule's own `checked` field,
and `crates/rd-siterules/tests/release_pack.rs` refuses a rule in the release file that
carries no probe its own `match` claims or a `checked` in the future.

**The run is triggered, never scheduled.** A download manager that went out and knocked on
board pages by itself would be doing something nobody asked it for. There is one command:

```bash
rdownloader doctor site-rules                 # every rule
rdownloader doctor site-rules --rule scnlog   # one of them, repeatable
```

It fetches each rule's `probe` through the same adapters the crawler selection uses -- the same
proxy profile and the same custom CA material this installation has stored -- one rule after
another, and prints a table:

```text
rule       state  links  reason
scnlog     ok     3      -
downmagaz  ok     2      -
gpaste     dead   0      site_rules.page_dead

3 rules checked, 1 not ok
```

The exit code is 1 as soon as one rule is not `ok`, so a release preparation can stop on it.
There is no captcha broker behind the one-shot command, which is the honest reading rather than
a gap: a page that cannot be reached without an answer nobody gave is `blocked`.

**Four states, and no fifth.** Every refusal the executor can produce sorts into one of them;
the refusal's own code is kept beside the state, so nothing is lost to the sort.

| State | What it means | From |
| --- | --- | --- |
| `ok` | Reachable, the structure fits, at least one link. | a successful run |
| `structural` | Reachable, nothing came back: theme or layout changed. | `no_links`, `structure`, `decode_failed`, `not_claimed`, `target_not_allowed`, and every limit |
| `blocked` | Something answered and refused. | `blocked`, `captcha_failed` |
| `dead` | Nothing answered, or the page is gone for good. | `page_dead`, `fetch_failed`, `address_not_public` |

A limit counts as `structural` although `RunError::is_limit` says a limit tells nothing about
the service: there are four states and a run that hit a limit still produced no links. The code
in the same row says which limit it was.

`target_not_allowed` is structural rather than dead -- the page still answers, it only leads
somewhere the rule does not know -- and `address_not_public` is dead, because the name no longer
points at the service but at a parked or local address.

**What the run stores, and who reads it.** One row per rule in `site_rule_checks`
(`crates/rd-db/migrations/0081_site_rule_checks.sql`), keyed by the rule's own id and holding
the state, the code, the number of links and the date of the run. Deliberately not a column on
`site_rules`: when the self-test arrived, a rule of the compiled-in pack had no row anywhere,
while a result had to exist for both kinds; since RD-130-07 every rule has a row, and the
separate table still means a run never rewrites a rule. The rule's own `checked` is **not**
rewritten -- it is its author's statement, for an imported rule the date the project measured
it before signing the file. `site_rule_checks.checked_at` is the run's date, and that is what
the rule list (RD-110-08) shows.

**A rule found `dead` is skipped, not deleted.** `serve` reads the dead ids at start and hands
them to the crawler selection through `SiteRules::set_dead`; a rule in that set costs no request
per paste. It keeps its place in the catalogue, it stays switched on, and the next run that
finds the service alive brings it back. Nothing deletes a rule because a site was down.

## User rules

A person's own rules live in the `site_rules` table (`crates/rd-db/migrations/0076_site_rules.sql`)
as the JSON above, with `name`, `rule_group` and `enabled` repeated outside the body so a
list can be drawn without parsing every rule. The database stores the body and does not read
it: validation belongs to the system boundary that accepts a rule (RD-110-08), which parses
it through `rd_siterules::Rule` before anything is written, and a dependency from `rd-db` on
the rule crate would rebuild the database and everything above it on every change to the
executor. Every write announces `site_rule.changed` (`Config` scope) naming the rule id and
never the body; a delete that removed nothing announces nothing.

**An id is taken once.** A second rule under an id the installation already holds is refused
with `site_rules.duplicate_id` — by the create, by the import and by `rd_siterules::Catalogue`
— never written over the first. `Catalogue` still refuses a user rule whose id a rule of its
shipped side carries (`site_rules.id_taken`); since RD-130-07 that side is empty.

**Duplicating a rule** (RD-130-07) is how somebody learns from a working one without touching
it. The settings page creates the copy through the ordinary `POST /api/v1/site-rules`: the
stored body unchanged, a free id (`<id>-copy`, then `<id>-copy-2`, shortened so it stays a valid
id) and a copy name, **switched off** — two rules claiming the same hosts would otherwise both be
consulted the moment the copy exists — and opens the copy in the editor. The original is neither
written nor switched.

## The settings page

*Settings → Site rules* (RD-110-08) is where both halves meet in front of a person:
`crates/rd-api/src/site_rules_handlers.rs` for the REST surface and
`web/src/components/settings/SettingsSiteRulesTab.vue` for the list. The list is grouped by
`group` and names, per rule, the hosts it claims and what the last self-test said — the verdict of the section above, with the refusal's own code in
the badge's title. Nothing here runs a self-test; that is `rdownloader doctor site-rules`, and the
page reads what it stored.

**Two switches, and where each one lives.** A rule keeps its switch in `site_rules.enabled`. A
group is not a rule, so its switch sits in `site_rule_switches`
(`migrations/0082_site_rule_switches.sql`, `scope` `group`, absence meaning on). One fact, one
home. A rule is consulted when its own switch and its group's are both on. Until RD-130-07 the
same table held, under `scope` `rule`, the switches of the compiled-in pack's rules, which had
no row of their own and could be switched but not edited; migration `0095` removed those rows
with the pack, and the service reads the `group` scope alone.

**The groups** (RD-130-07). The release file uses five: `board`, `paste`, `ebooks`, `graphics`
and `adult`, each with a label in all four languages (`web/src/locales/*/siterules.json`).
`comics` and `magazines` were merged into `ebooks`, and migration `0095` moved stored rules of
those groups — the column and the body alike — and their group switches with them; off wins, so
`ebooks` is off afterwards if any of the three was. `graphics` stays a group of its own: 3D and
graphics assets are not books. A group a person invents is shown as the raw
word, because the list is open.

**One assembly, three callers.** `rd_api::site_rules_service::catalogue` turns the stored rules
and those switches into the catalogue in force. `serve` uses it at start, `rdownloader doctor
site-rules` uses it, and every write from this page uses it and hands the result to
`rd_plugin_ext::SiteRules::replace`, so a change takes effect on the next paste rather than after
a restart. A fresh installation holds no rule, so the catalogue is empty until something is
imported or written.

**The trial run** (`POST /api/v1/site-rules/test`) takes a rule body that need not be stored and
an address the person names, runs it exactly as the selection would, and reports the links, the
package name and the number of requests. Each link is then judged by
`crate::collector_crawl_verdict`, the same function a real paste applies (RD-110-07), so the trial
cannot disagree with what the intake will later do; an address that answers with a page carries
`collector.crawl_not_a_file`. Unlike the `doctor` command it is given the captcha broker, because
a person triggered it in front of an interface that can show them the challenge.

**Import is a security boundary.** A rule from a file is untrusted input in the strict sense: it
names hosts to fetch and patterns to run. Every body goes through `rd_siterules::Rule` and
`Rule::validate`, the same gate a rule from the pack passes; a body that fails and one that
repeats an existing id are each refused on their own, with their own code, rather than failing
the whole file. What survives is stored **switched off**, always — the signed release file's
rules included, because a signature says who wrote a rule, not that this person wants it —
`POST /api/v1/site-rules/import` has no field that could ask for anything else, so activation is a
separate `PUT /api/v1/site-rules/{id}/enabled` per rule. The confirmation is therefore a property
of the service, not of the interface: a client that never drew a dialog still activates nothing.
The export (`GET /api/v1/site-rules/export`) writes the installation's rules under
`format_version` — deliberately not a `RulePack`, because a file somebody was sent is not a
signed document and naming it one would invite the two to be confused exactly where the difference
matters.

**The import takes both kinds of file** (RD-130-07) and tells them apart by the envelope: a
body with `signatures` is the signed release file and is verified as one — against the
compiled-in `Role::SiteRules` root, from the request's bytes exactly as they arrived, since the
signature covers those bytes and a parsed copy would not be what was signed — and anything else
is read as an export. The response says which (`signed`). The interface therefore sends the
file's text unparsed. A signed file that fails a check is refused whole, with the codes under
"Trust", and is never read a second time as an export.

## Publishing the rule file

```bash
rdownloader plugin keygen --role site-rules --output ~/.config/rdownloader   # once
rdownloader site-rules sign --input crates/rd-siterules/resources/site-rules-payload.json \
    --output crates/rd-siterules/resources/site-rules.json \
    --key ~/.config/rdownloader/rdownloader-siterules.key
rdownloader site-rules verify crates/rd-siterules/resources/site-rules.json
```

`sign` refuses a payload with a foreign `format_version`, an invalid rule or a repeated id,
so what is signed is what the build will accept; `verify` runs the import's own check and names
the rule count, the sequence and the groups. The public half of the key is the
`Role::SiteRules` entry in `crates/rd-sign/src/roots.rs`; rotating it is an entry with an
overlap window, as for every other root. `crates/rd-siterules/tests/release_pack.rs` holds
the committed file to the compiled-in root, and `tests/rules.rs` holds it to the payload, so a
file edited without being re-signed fails the build's tests rather than the person's import.

**Since RD-130-07 the file is a release artifact, not part of the binary.** It is signed
locally — CI holds only the plugin key — and committed. `.github/workflows/release.yml`
verifies the committed file with the release build and attaches it to the release as
`rdownloader-site-rules.json`, beside the packages; locally `scripts/package-linux.sh` does the
same check with the binary it just built and writes `artifacts/rdownloader-site-rules.json`,
which `scripts/release-pipeline.sh`'s artifact step requires. Offering the file for download
on the website is the owner's part.

The unsigned payload is kept beside the signed file as
`crates/rd-siterules/resources/site-rules-payload.json`, so the file is re-signed from what was
last published rather than reconstructed by hand.

## The services the release file recognises

Sequence 6, eight rules. Every one of them was opened on the live service with a current
Chrome user agent, no account and nothing solved, and carries that date as its `checked`. The
group is what the interface switches by; sequence 6 (RD-130-07) changed nothing but the groups
of `downmagaz` and `getcomics`, now `ebooks`.

| Rule | Service | Group | Hosts it claims | Measured |
| --- | --- | --- | --- | --- |
| `scnlog` | scnlog.me | `board` | `scnlog.me`, `*.scnlog.me` | 2026-09-22 |
| `downmagaz` | downmagaz.net | `ebooks` | `downmagaz.net`, `*.downmagaz.net` | 2026-09-22 |
| `paste-generic` | ControlC | `paste` | `controlc.com` | 2026-09-22 |
| `getcomics` | GetComics | `ebooks` | `getcomics.org` | 2026-09-22 |
| `scene-rls` | Scene-RLS | `board` | `scene-rls.com`, `scene-rls.net` | 2026-09-22 |
| `avaxhome` | AvaxHome | `ebooks` | `avxhm.se`, `avxhome.st`, `xsava.xyz`, `zavat.pw` | 2026-09-22 |
| `cgpersia` | CGPersia | `graphics` | `cgpersia.com` | 2026-09-22 |
| `vipergirls` | ViperGirls | `adult` | `vipergirls.to`, `viper.to` | 2026-09-22 |

**Two rules left the pack on that day** (RD-120-17). `libgen` (Library Genesis) was removed
because libgen.bz no longer resolves the address its rule reads out of the mirror row, and
`satdl` (satdl.com) because the project owner withdrew the service. A rule that does not
deliver is worse than no rule: the address is claimed, the generic crawlers never see it, and
the refusal reads as a defect in rDownloader. Their recorded pages and cases went with them.

Seventeen further domains these services once used are carried as `dead`, so an old address is
rewritten rather than refused: `scnlog.eu`, `scnlog.life`; `pasted.co`, `tinypaste.com`,
`tny.cz`, `binbox.io` with their `www.` forms and `www.controlc.com`; `getcomics.info`;
`avxhm.in`, `avxhm.is`, `avaxhome.bz`, `avaxhome.ws`, `avh.world`.

From RD-120-17 to 1.2 the settings page showed that date as "Checked" for a shipped rule with
no local self-test result. Since RD-130-07 an imported rule is the person's own, and its
`checked` is its author's word rather than a measurement of this installation's reach, so every
rule without a local self-test reads "Not checked".

**Four services on RD-110-11's candidate list carry no rule, and why is the result.** They are
recorded in `docs/roadmap/jobs/110-11-regelpaket-fuellen.md` with the measurement:
`hdencode.org` keeps no address in its HTML and hands the links out behind a reveal form with a
Cloudflare Turnstile widget and an image-captcha fallback, whose submission was not measured;
`metalarea.org` and `hi10anime.com` replace the link block with a registration notice for a
guest, and no rule is written against a credential this project does not have; and `13dl.to`,
the service's only domain, now answers with a parking page.

## Writing a rule for a real page

What the first real sites taught, all cheap to get wrong (RD-110-10, RD-110-12, RD-110-13,
RD-110-11) — and, for the two limits those measurements found, what RD-120-12 decided about
them (`docs/adr/0017-the-rule-format-keeps-two-limits.md`).

**A container that spans lines needs `(?s)`.** The Rust `regex` crate does not let `.` match a
newline, and the block a release page keeps its links in almost always spans several. Without
the inline flag the container step matches nothing and the run refuses with
`site_rules.structure` — a structural failure that is really a mistake in the pattern. Both
rules in the release file carry it:

```jsonc
{ "kind": "regex", "pattern": "(?s)<div class=\"download\">(.*?)</div>", "into": "container" }
```

**A rule cannot say "this host, no" — but it can say "these hosts, yes".** There is no filter
step, and the Rust `regex` crate has no lookahead, so a *negative* host pattern is not available.
The positive one is, and costs nothing: the `regex` step with `all` keeps only what its pattern
matches, so an alternation naming the accepted hosts is already an allow-filter, and `satdl` uses
exactly that (`https?://satdl\.com/product/[0-9]+/...`). Use it wherever the accepted hosts are
known in advance. Where they are not — a release board links to whatever hoster the uploader
chose — what keeps a page's own links out of the package is the *container*, not a filter: cut
out the block that holds only the hoster links, and the navigation never enters. Most rules in
the release file work that way, and the scnlog release page carries sixteen links back to itself that none
of them reaches.

**RD-120-12 decided that the format does not grow a `drop` step for the rest**, and
`docs/adr/0017-the-rule-format-keeps-two-limits.md` carries the reasoning. In short: the page
that produced this limit did not survive re-measurement (see the next paragraph), the positive
form above covers the cases where the hosts are known, and a rule-level drop would remove the
stray address *and* the count that announces it. A `keep` step reopens the question, but only
behind a measured page whose own links are genuinely interleaved with the hoster links in
numbers.

**A container that catches one wrong address is not a reason to refuse the rule.** RD-110-11
measured this rather than assuming it. Two of the ten containers carry exactly one address that
is not a file: the GetComics theme puts its "read online" button in the same `aio-button-center`
block as the hosters, and one scene-rls page of eight put the site's own NFO viewer in the
centred `h2`. Neither can be excepted, and neither has to be: RD-110-07's crawl verdict probes
an unclaimed address, sees a page, refuses it with `collector.crawl_not_a_file` and *counts* it,
so the person reads "1 of 6 found links was not a file" instead of meeting a queued HTML page.
That is the division of labour — the container is the rule's filter, the verdict is the safety
net — and a page whose links are genuinely interleaved with its own is still out of reach.
That last sentence is the condition under which RD-120-12's decision reopens, and nothing
measured so far meets it.

**A host that only forwards belongs in `dead`, not in `match`.** `dead` rewrites an address to
the canonical host instead of refusing it, and that is exactly what a domain does that answers
every path with a 301 to another one. `paste-generic` puts all nine such hosts there —
`pasted.co`, `tinypaste.com`, `tny.cz`, `binbox.io` with their `www.` forms, and
`www.controlc.com`, whose origin server answers 522 — and the rule then reaches the paste in one
request instead of two, and keeps working if one of those registrations lapses. The price is
that `match.hosts` may carry no `*.` wildcard for the same domain: the validation refuses a host
that is claimed as live and listed as dead, and it is right to.

**Claim narrowly; an unclaimed address is not a refused one.** A service's own pages sit on the
service's own host, and there is no lookahead with which to except them. `paste-generic` claims
`^/[a-f0-9]{8}$`, the shape every measured paste identifier has, so `/login`, `/register`,
`/terms` and the front page are simply not claimed — and an address a rule does not claim falls
through to the next source in the selection, which is the safe direction. A wide pattern plus a
refusal would stop the search at the rule.

**A rule cannot walk a list.** RD-110-13 measured `serienjunkies.org`, whose HTML carries no
download address at all and whose episodes arrive by XHR. Everything about it is readable —
`GET /api/media/<id>/releases` answers JSON to anyone, no captcha, no account — and a rule still
cannot describe it, for two reasons that are worth knowing before starting such a page:

- **A template takes the first value of a list, not each of them.** `${name}` over a list
  expands to its first string (`Variables::expand`). Only `decode` and `redirect` walk a list;
  no step makes one request per element against a templated address. So a page whose address
  yields *N* identifiers, each needing its own request, is out of reach — and so is "one package
  per item", since `package` is one source read once.
- **A pattern is a literal, not a template.** `regex.pattern` is compiled as written, so a rule
  cannot search a list for a value it read from the page.

**RD-120-12 decided that the format does not grow a loop step yet**, and
`docs/adr/0017-the-rule-format-keeps-two-limits.md` carries the reasoning beside the filter
decision above. In short: the only page that asked for it wants a browser fingerprint in the
request body as well, so a loop would not make it reachable; 39 releases and 78
release-and-hoster pairs do not fit `max_pages` = 24 either, so the step would arrive with an
unargued budget increase attached; and `Step::Redirect` already walking a list means this is
cheap to add *when a page needs it*, not a reason to add it before one does. The question reopens
behind a measured page that needs one request per element and nothing else this format refuses.
This limit is recorded with the page that produced it:
`docs/adr/0013-serienjunkies-a-list-a-rule-cannot-walk.md` and
`docs/roadmap/jobs/110-13-serienjunkies-ohne-links-im-html.md`. Until then a service that
delivers its releases by XHR carries no rule, and neither `serienjunkies.org` nor its identical
sister `dokujunkies.org` is in the release file.

**A gateway on the service's own host is two more steps, not a wall.** Three of the rules
RD-110-11 added did not find the hoster address on the page at all: `avaxhome` reads a
`/go/<token>` address out of the download block, and the two that RD-120-17 removed read an
`ads.php` address out of a mirror row (`libgen`) and walked a product page to a download page
(`satdl`). Each of those is on a host the rule already claims, so `host_allowed` passes; a
`fetch` with a relative address joins against `page_url`, and a `redirect` step at the end keeps
the target without fetching it, which is what
lets the answer be on any host. The cost is one request per hop against `max_depth` and
`max_pages`, and the whole shape fits inside the defaults with room to spare. What it does *not*
survive is a token that is bound to the requester — `avaxhome` puts the caller's own address in
its `/go/` token, so the address is read from the page each run rather than built from a
template.

**Say what the page is before choosing `mirrors`.** `cgpersia` lists five archive parts at three
hosters — fifteen addresses, five files — and leaves the field out, because one group for the
page would fold five files into one. `getcomics` lists one comic at five hosters and sets it.
The question is never "are there several addresses" but "is there one file behind them".
