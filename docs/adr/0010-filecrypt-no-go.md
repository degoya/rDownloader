# ADR 0010 — No filecrypt.cc crawler

- **Status:** Accepted
- **Date:** 2026-09-21
- **Job:** RD-110-16
- **Supersedes:** —

## Context

RD-110-16 asks for a `plugins/filecrypt-crawler/` on `world crawler-plugin` that resolves a
filecrypt folder to the hoster links behind it: the offered DLC container first, the per-line
resolution second, reCAPTCHA v2 / CutCaptcha / click-point as the page demands, the folder
password asked of the person, and four distinguishable refusals. RD-110-15 delivered the two
captcha kinds the contract was missing precisely so that this job could be done.

The job's own starting position, measured 2026-09-20, recorded only that the three domains
answer `200`. Nobody had yet looked at what a **container page** answers. The feasibility
measurement of 2026-09-21 did, and found a different service from the one the job was cut
against.

### What was measured, 2026-09-21

All probes with `curl -A 'Mozilla/5.0 (Windows NT 10.0; Win64; x64) … Chrome/130.0 Safari/537.36'`,
redirects followed, no account, no session taken from a browser, nothing solved.

1. **The domains live.** `filecrypt.cc`, `filecrypt.co` and `filecrypt.to` each answer `200`.
   That part of the job's premise still holds.

2. **Every container page is an anti-bot interstitial, not a folder.** Three container
   identifiers found in public sources — `1A87CFB756`, `3CC24754F6`, `D46E52C360` — were
   requested on all three domains. All nine responses are `200` with 18 032–18 975 bytes, and
   all nine are the same page: `<h2>Security Check</h2>`, "To continue, please confirm you're
   not a robot.", `<div class="pow-captcha" data-state="idle">` and
   `<noscript>Please enable JavaScript to continue.</noscript>`. No file list, no hoster link,
   no captcha of a kind the plugin contract knows, and no password field — the password input
   is present in the markup but **commented out**, because the gate sits in front of that
   stage.

3. **The gate is a proof-of-work with behavioural evidence.** The form posts back six hidden
   fields: `pow_id`, `pow_nonce`, `pow_elapsed`, `pow_pauses`, `pow_data`, `pow_x`. The
   session endpoint named in `data-session` answers plainly:

   ```
   GET /captchasession/B89458A4CC.json
   {"success":true,"autosolve":false,"challenge":{"id":"CA37F76C8C",
    "challenge":"704495071a11a19b8e038cd5dc5dc5c2","difficulty":20}}
   ```

   `/js/pow_captcha_worker.js` (3 248 B) is a hand-rolled SHA-1 with a leading-zero search —
   a hashcash at 20 bits. That part alone would be mere cost. The other four fields are not:
   `pow_elapsed` and `pow_pauses` are timing and interaction evidence about how a human used
   the widget, and `pow_x`/`pow_data` come from the two modules the element names in
   `data-sig` and `data-ext` — `/js/s.js` (77 645 B) and `/js/m.js` (61 008 B), both
   obfuscated past the point where any identifier survives. `data-px` points the widget at
   `https://v3-eu.cutcaptcha.net/`, `https://pow.filecrypt.cc` and
   `https://captcha.filecrypt.cc`.

4. **The container route — the one the job calls the stablest — returns nothing.**
   `/DLC/1A87CFB756.dlc` and `/DLC/1A87CFB756.html` both answer `200` with **0 bytes**.
   `POST /Container/1A87CFB756.html` with `DownloadDLC=1` answers `200` with the interstitial
   again. rDownloader's DLC reader (`crates/rd-collector/src/dlc.rs`,
   `crates/rd-api/src/dlc_import.rs`) is not the missing piece; there is simply no container
   to hand it.

5. **The gate does not lift.** Three requests in one session, `PHPSESSID` and `lang_v2` carried
   forward, returned the byte-identical interstitial each time.

6. **The page carries a bot trap.** `<a href="/Link/1" style="display:none;"></a>` — a link no
   person can click. Following it is how an automated client identifies itself.

7. **Nothing of the folder leaks past the gate.** The gated page contains exactly three
   `/Link/` occurrences: the `openLink` JavaScript template, the hidden trap above, and
   `/Link/94FACF48F7.html`, which is the popunder target of an advertising redirect
   (`linkonclick.com/jump/next.php`), not a folder item. The per-line route is not reachable
   without identifiers that only the passed gate yields.

8. **The operator says no in writing, to every path.**

   ```
   GET /robots.txt
   User-agent: *
   Content-signal: search=no,ai-train=no
   Disallow: /
   ```

   Not a section, not a directory — the whole site, `/Container/` included.

### What could not be measured, and why that matters

The job asks for four distinguishable refusals. Only one is observable from outside the gate:
an unknown container identifier answers `302` to `/404.html`, which answers `404` with
6 291 bytes. **Wrong password, failed captcha and empty folder all sit behind the gate**, and
so does the question of which captcha kind the page serves in 2026 at all. The `cutcaptcha`,
`g-recaptcha` and `circle_captcha` names still appear in the page's stylesheet block, which
says those stages still exist somewhere behind the check — but a stylesheet is not a
measurement, and this record does not claim to have seen one.

That is the decisive point for the acceptance criteria. Every test the job asks for would be
written against a fixture nobody has ever recorded from the live service: a synthetic guess at
a page shape, dressed as a recording.

## Options considered

### 1. Implement the proof-of-work in the plugin — rejected

Computing 20 bits of SHA-1 is easy and would be defensible on its own. It is not what the gate
asks for. Passing it also requires executing `s.js` and `m.js` to produce `pow_x`/`pow_data`,
and supplying `pow_elapsed` and `pow_pauses` — numbers that describe a human's hesitation at a
widget. A plugin that computes them is fabricating a browser fingerprint and faking interaction
evidence. RD-110-16's own brief forbids it, RD-110-14's "Out of scope" forbids it for the whole
class ("Kein Fingerabdruck-Nachbau, kein Lösen einer Challenge durch Dritte"), and the
feasibility gate carried forward from milestone 1.0.3 prefers a documented No-Go to brittle
scraping. There is no reading under which this is in scope.

### 2. Ask the gate through the captcha contract — rejected, it does not fit

`captcha-challenge` has six cases: three widget kinds with a sitekey and a page address, an
image, a click-point, and a CutCaptcha with two keys. A proof-of-work over a site's own
obfuscated modules is none of them. There is no image to show a person, no sitekey any solver
service addresses, and nothing the browser extension could read a token out of. Adding a
seventh case for "run this one site's JavaScript" would not be a captcha kind; it would be a
remote code execution facility with a site name on it. RD-109-11 removed a browser runtime
(`wry`) from the plugin platform a milestone ago for less.

### 3. Read the page in the person's own browser via the extension — rejected here

This is RD-110-14's third outcome and it is a real mechanism, but it is not this job. The job
puts Click'n'Load explicitly out of scope: "CNL landet beim Capture-Agenten, nicht im Crawler.
Der vorhandene Weg bleibt, wie er ist." And filecrypt's own page offers Click'n'Load, which the
capture agent already receives (`crates/rd-capture/src/cnl.rs`). The person who opens the
folder in their browser, passes the check the operator put there for them, and presses CNL is
already served by rDownloader today. Building a crawler that pretends to be that browser adds
nothing for that person and breaks the moment the modules are rotated.

### 4. Ship the plugin against synthetic fixtures and call it done — rejected

The DLC path returns zero bytes, the per-line path is unreachable, and no captcha stage was
observed. Every one of the seven acceptance criteria would be satisfied against a page shape
invented for the purpose. That is a plugin whose tests prove only that it is self-consistent,
a signed component claiming three domains it cannot resolve, and a provider entry that makes
the LinkGrabber stop offering the route that does work. RD-110-07 exists to stop exactly this
sort of confident nothing.

## Decision

filecrypt.cc gets no crawler plugin in 1.1. RD-110-16 is `Blocked/No-Go` as of 2026-09-21. No
`plugins/filecrypt-crawler/` directory, no manifest, no crate, no provider or domain entry, no
`claims-url` for `filecrypt.cc`, `filecrypt.co` or `filecrypt.to`. The bundled component count
does not change.

RD-110-15 is **not** retroactively wasted: `click-point` and `cutcaptcha` are in the contract,
tested, and are what RD-110-17's remaining protectors will be measured against. What this
record says is that filecrypt is no longer the service that justified them.

The case reopens when any one of three things is true, and the job file names all three:

1. filecrypt serves container pages without the proof-of-work interstitial again, or gates them
   with a challenge the contract already expresses — a reCAPTCHA, a CutCaptcha, a click-point
   image — that a solver service or the person can answer.
2. The operator offers a documented route for download clients, and `robots.txt` stops
   disallowing every path.
3. The project decides, as a separate piece of work with its own record, that the extension may
   read a gated page in the person's browser and hand the links over — RD-110-14's third
   outcome, generalised. This record does not decide that question and does not prejudge it.

## Consequences

- `docs/roadmap/jobs/110-16-filecrypt.md` carries the measurement of 2026-09-21, the status and
  the three reopening conditions. No acceptance box is ticked: none of them was met, and a
  No-Go is not a way of meeting them.
- Pasting a `filecrypt.cc/Container/…` address keeps its current behaviour — no plugin claims
  it, so link intake treats it as an unknown site. Combined with RD-110-07 that is a refusal
  with a code, not an HTML file enqueued as a download.
- **The working route is untouched and is the one to point people at:** open the folder in a
  browser, pass the operator's check, and press Click'n'Load. The capture agent receives it
  (`crates/rd-capture/src/cnl.rs`), and a DLC file downloaded by hand is imported by
  `crates/rd-api/src/dlc_import.rs`. Neither needed this job and neither is affected by it.
- RD-110-17 ("Die übrigen Protektoren") inherits a sharper question than it was cut with. Its
  seven services must each be measured at the **container page**, not at the domain root:
  filecrypt answered `200` at the root on both measurement days while being unresolvable
  throughout. The liveness rule needs that refinement written into it, and RD-110-00 records it.
- filecrypt now belongs to the class RD-110-14 investigates. Its finding is recorded here
  rather than moved there, because RD-110-14's five services are named in its own text and this
  one arrived from a different direction.
- Nothing about the plugin contract, the captcha kinds, the provider registry or the DLC reader
  changes. In particular this record does **not** ask for a seventh `captcha-challenge` case,
  and it does not ask for a JavaScript runtime in the plugin host.
- What is **not** claimed here: that filecrypt folders cannot be downloaded with rDownloader.
  They can, by the route above. What is claimed is that no *unattended crawler* route exists
  that this project may build, can verify against a recorded response, and could keep working.
