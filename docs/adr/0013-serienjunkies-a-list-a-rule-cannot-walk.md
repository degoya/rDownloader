# ADR 0013 — No rule for serienjunkies.org: a list the format cannot walk

- **Status:** Accepted
- **Date:** 2026-09-21
- **Job:** RD-110-13
- **Supersedes:** —

## Context

RD-110-13 named `serienjunkies.org` as the first service whose HTML carries no download address
at all, and asked for the proof that `fetch-json` and the `captcha` step carry such a page. Its
sister site `dokujunkies.org` was to follow the same way, or else go to RD-110-11 as a rule of
its own.

This is **not** the case ADR 0012 records. Nothing here is gated. Every measurement below came
back from the site's own application — `x-powered-by: Express`, never `cf-mitigated` — and the
episode list is served to anyone who asks, without a captcha, a cookie or an account. The site
is entirely readable. What cannot be written is the rule.

### What was measured, 2026-09-21

All probes with `curl -A 'Mozilla/5.0 (Windows NT 10.0; Win64; x64) … Chrome/140.0.0.0 Safari/537.36'`,
no account, no session, nothing solved, Cloudflare colo `MUC`. The full table is in
`docs/roadmap/jobs/110-13-serienjunkies-ohne-links-im-html.md`.

1. **The page is a shell, and it names its own data.** `/serie/alien_-earth` answers 200 with
   8 191 bytes and not one download address. It carries a single empty anchor with
   `data-mediaid`, `data-mediatitle` and `data-captchasitekey`, and loads
   `/dist/medium/releases.js` — 4 324 477 bytes of **unminified** webpack output in which the
   client's whole API module is readable source.

2. **The episode list is free.** `GET /api/media/<mediaId>/releases` answers 200,
   `application/json`, no captcha, no account. Its shape is an object keyed by season (`S1`,
   `SP`), each holding `items`, an array of release objects with `id`, `name`, `season`,
   `episode`, `resolution`, `group` and `hoster`. For the one series measured: 39 releases,
   8 distinct episodes, 78 release-and-hoster pairs.

3. **The captcha sits on the link handout, not on a redirect.**
   `POST /api/releases/<releaseId>/downloads/<hoster>` with a JSON body of `recaptchaToken` and
   `fphash`. Every `POST` without a solved reCAPTCHA answers `403` with an empty body — JSON or
   form-encoded, with or without `Referer` and `Origin`. `GET` and `PUT` answer 404, `OPTIONS`
   answers `POST`. The response to a solved request is a JSON array of `{ "url": … }`: the hoster
   address itself. **There is no forwarding address, so there is no `redirect` step to write** —
   the job's planning line assumed one the site does not have.

4. **The second body field is a browser fingerprint.** `fphash` is
   `Fingerprint2.x64hash128(Fingerprint2.getPromise() …, 31)` — fingerprintjs2 over canvas,
   WebGL, fonts, audio, screen, timezone and plugins.

5. **There is no route around the application.** `/feed`, `/rss` and `/sitemap.xml` answer 404.
   `robots.txt` answers 200 but holds only Cloudflare's content-signal preamble — no `Disallow`,
   no `Allow`, no `Sitemap`.

6. **`dokujunkies.org` is the same application.** Same bundles, same `/list/` navigation, same
   `data-mediaid` anchor, same `/api/media/<id>/releases` shape, a different reCAPTCHA sitekey.
   It is not a second rule; it is the same finding twice.

### What the format cannot do, checked against the crate

Each of these was read in `crates/rd-siterules/`, not inferred from the documentation.

- **A template takes only the first string of a list.** `Variables::expand`
  (`src/exec/value.rs`) replaces `${name}` with `Value::first`, and its own test
  `a_template_takes_the_first_string_and_refuses_an_unwritten_name` says so. Only `decode` and
  `redirect` walk a list, and neither makes a request per element against a templated address.
  A season address therefore cannot become 39 requests. **This alone ends the job's outcome.**

  What that costs to lift is smaller than it reads, and the successor job should know it:
  `Step::Redirect` (`src/exec/steps.rs`) already walks a list and issues **one request per
  element**, one after another, collecting the answers into a new list. So looping requests is
  not a capability the executor lacks — it is a shape it already has. What is missing is only
  that the address be built from the element through a template instead of *being* the element.
  A loop step is therefore a generalisation of something already written and tested, not a new
  machine.
- **`regex.pattern` is not a template.** `Step::Regex` hands the pattern straight to
  `Regex::new` (`src/exec/steps.rs`); loading checks it with `check_pattern`, not
  `check_template` (`src/step.rs`). So the fallback shape fails too: a rule on the per-release
  address `/serie/<slug>/<releasename>` reads the release *name* from its own page but cannot
  search the season list for it, and the release *id* appears nowhere in the HTML.
- **A run yields one package name.** `package` is a single source read once, so "one package per
  episode" is unreachable even if everything else worked.
- **`fetch-json` refuses an array of objects.** `json_at` (`src/exec/steps.rs`) turns a JSON array
  into a list only when every element is a scalar. No pointer reaches the ids. This one is
  avoidable — `fetch` reads the body as text and `regex` with `all` would take them — but it
  means the step the job set out to prove is not the step this page needs.
- **`form` sends `application/x-www-form-urlencoded` and nothing else** (`src/exec/ports.rs`),
  while the endpoint wants JSON. Whether Express also accepts urlencoded here could not be
  measured: without a solved captcha both shapes answer the same 403. That gap is recorded
  rather than guessed, and it changes nothing.

## Decision

**No rule for `serienjunkies.org`, and none for `dokujunkies.org`.** The shipped pack
(`crates/rd-siterules/resources/site-rules.json`) stays at sequence 2 with `scnlog` and
`downmagaz`. A rule that cannot walk a season would be a rule written against a page it cannot
read to the end, and RD-110-10 already set the standard that an invented rule is worse than none.

**No workaround is built.** Not a per-release rule that stops before the list lookup, not a
fingerprint, not a JSON body bolted onto `form` for one site. The first would ship a rule that
refuses; the second is the line ADR 0010 drew and ADR 0012 kept; the third is useless without
the loop that does not exist.

## Consequences

- `docs/roadmap/jobs/110-13-serienjunkies-ohne-links-im-html.md` carries the measurement, the
  status `Blocked/No-Go`, and the acceptance boxes answered one by one — the investigation ticked,
  the rule-shaped ones refused with their reason.
- `dokujunkies.org` leaves RD-110-11's candidate list. It was never a separate pattern.
- **The rule format now has two measured limits, each behind a real page**, and they belong
  together in whatever succeeds RD-110-04:
  1. "this host, not that one" — RD-110-10, `scene-rls.net`. There is no filter step and Rust
     `regex` has no lookahead.
  2. "for every value of this list, do these steps" — this record. Without it every service that
     delivers its releases by XHR is out of reach, and `serienjunkies.org` is only the first one
     measured.
  `docs/site-rules.md` states both under "What a rule cannot say", so the next person meets them
  before writing a rule rather than after.
- **No new error code, and none was needed.** Nothing was shipped that could fail. Had a rule
  been written, the codes it would have produced already exist: `site_rules.structure` for the
  JSON shape, `site_rules.captcha_failed` for the broker, `site_rules.fetch_failed` for the 500 a
  missing series answers with.
- **What is not claimed here:** that the site is defended, that it is dead, or that it dislikes
  this project. It is none of those. It is an ordinary single-page application whose data a rule
  may read and may not iterate.

The case reopens when any one of these is true:

1. The rule format gains a step that repeats other steps for every value of a list, and a way to
   carry a value into a pattern. Both are needed; either alone leaves this page unreachable.
2. `serienjunkies.org` serves its hoster links without a browser fingerprint — the reCAPTCHA
   alone the contract already expresses, through `rd-captcha` and the `captcha` step.
3. The project decides, as separate work with its own record, that the extension may read such a
   page in the person's own browser and hand the links over. That is the same open question
   ADR 0010 and ADR 0012 left, and this record neither takes it nor prejudges it.
