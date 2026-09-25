# ADR 0018 — No IcerBox resolver, although the API is clean

- **Status:** Accepted
- **Date:** 2026-09-22
- **Job:** RD-120-19
- **Supersedes:** —

## Context

RD-120-19 asks for a resolver for the hosters the shipped site rules hand over to. `icerbox.com`
is the first of them: the `avaxhome` rule (`avxhm.se`, probe
`https://avxhm.se/ebooks/3032034086E.html`) resolves correctly and produces an `icerbox.com`
address that no installed plugin claims.

Measured on 2026-09-22 under the wave-0 gate (RD-120-04, 07–10) with RD-110-16's method.

DNS healthy first: a guaranteed-nonexistent name answered NXDOMAIN immediately, `example.com`
resolved immediately, and `icerbox.com` resolves to `188.114.97.3` / `188.114.96.3`
(Cloudflare, NS `jeff`/`mary.ns.cloudflare.com`). Nothing here is a slow-resolver artefact.

### What was measured, 2026-09-22

1. **A real file page, not the domain root.** `avxhm.se`'s download button is a `/go/<token>`
   redirect; followed with the item page as referer it lands on
   `https://icerbox.com/lWBye5en/3032034086.epub`, `200`, 15 808 bytes.

2. **`robots.txt` is permissive.** `User-agent: *` / `Disallow:` — empty, so nothing is
   disallowed. On this criterion alone IcerBox would pass, which is why it is not the criterion
   that decides.

3. **The file page carries no file.** It is an AngularJS single-page application: the served
   HTML is the shell, every visible string is a `{{ … | translate }}` placeholder, and there is
   no file name, no size and no download link in it. `https://www.google.com/recaptcha/api.js` is
   loaded on the file page itself.

4. **There is a clean JSON API, and it is private.** `static/js/config.js` gives
   `API: "https://icerbox.com/api/v1/"`; the bundle names `auth/login`, `auth/refresh`,
   `dl/free/step1`, `dl/free/step2`, `filemanager/info/`, `filemanager/ls` and the rest. It
   answers properly — `POST /api/v1/dl/free/step1` with no body returns
   `{"message":"422 Unprocessable Entity","errors":{"file":["The file field is required."]}}`.
   No documentation is published for it: `/api`, `/api/docs` and `/api/v1` are `404`,
   `docs.icerbox.com` does not resolve, `/developers` is the SPA's not-found shell, and the only
   statement about it is the support page's `SUPPORT.C1_A2`, "Yes, we offer developers the
   possibility to use IcerBox on their own projects", with nothing to follow.

5. **Free downloads are off on every file the rule actually produces.** `POST /api/v1/dl/free/step1`
   with `{"file":"<slug>"}` answers `{"message":"The owner of this file disabled free
   downloads.","status_code":403}` for four of four `avxhm.se` items resolved this way
   (`lWBye5en`, `lWBymwwn`, `nE0zqPyO`, `lKoK5NBl`). This is not incidental: `avxhm.se` runs an
   IcerBox affiliate banner — *"Wir arbeiten nur mit IcerBox.com, klicken Sie hier, um sich
   anzumelden!"* — whose `cutt.red` shortener lands on `https://icerbox.com/premium?ref=LzVL09`.
   The board earns on premium sign-ups, so its own uploads have the free route switched off.

6. **The service says it has no free tier.** Its terms, `TOS.P7`: "We provide Only premium
   subscription services."

7. **The terms forbid exactly what a resolver does.** `TOS.P14`: "you will not, and will not
   allow any third party to: … (ii) use any data mining, robots or similar data gathering or
   extraction methods with respect to the Site or the Service; (iii) download (other than the
   page caching) any portion of the Site". `TOS.P26` forbids "script or other software designed
   to automate any functionality on the Service without {{ domain }}'s express written consent".

## Decision

**No IcerBox resolver.** RD-120-19 records `icerbox.com` as a measured No-Go.

Three findings would each have been enough, and they point the same way:

- the only route a resolver could walk is an undocumented private API that the operator's own
  terms forbid automating, in the plainest words any service measured so far has used;
- the files the shipped rule actually hands over cannot be fetched without a premium account,
  by the uploader's deliberate setting, so a free-flow resolver would resolve to a `403` every
  time;
- the service states it sells premium subscriptions only, so there is no free tier for the
  resolver to be written against.

A credentialed resolver — the shape `keep2share`, `rapidgator` and `nitroflare` have — is the
only technically open variant, and `TOS.P26`'s "without express written consent" closes it too.
This is not the usual friction of an unpleasant interface; it is the operator declining.

## Consequences

No code, no plugin directory, no provider entry, no manifest change; the bundled component
count does not change.

`avxhm.se` keeps its rule. The rule is correct and this record does not touch it: it finds the
address it is supposed to find, and the address belongs to a hoster this application will not
serve. What the person sees at the end of that chain is RD-120-18's subject, not this one's.

The `robots.txt` was permissive and the API was clean, and neither mattered. That is the
reusable part: `robots.txt` can only refuse, never permit, and a well-built interface says
nothing about whether its operator wants it used. Read the terms.

If IcerBox publishes a developer API with terms that allow a download client to use it, this
record is superseded rather than edited.
