# ADR 0019 — The address a board shows is not always the hoster

- **Status:** Accepted
- **Date:** 2026-09-22
- **Job:** RD-120-19
- **Supersedes:** —

## Context

RD-120-19 was cut from a finding: three addresses that shipped site rules hand over to have no
resolver, so a rule that works produces nothing. Two of them are `nfile.cc` and `dwp.la`, both
via `downmagaz.net`; the third, `icerbox.com`, is ADR 0018's subject. ViperGirls was named as
leading to unsupported hosts without saying which, and measuring that was the first task.

Measured on 2026-09-22 with RD-110-16's method, DNS checked first (NXDOMAIN immediate,
`example.com` immediate).

### What was measured

**`nfile.cc` and `dwp.la` are not hosters.** The `downmagaz` probe page
(`https://downmagaz.net/business_magazine_economics/484259-the-economist-usa-09192026.html`)
carries exactly two links, both images in a `<div align="center">`:
`https://nfile.cc/qK7XDAwq` and `https://dwp.la/d/dro`. Followed:

| Address | Result | Real target |
| --- | --- | --- |
| `https://nfile.cc/qK7XDAwq` | `200` after two redirects | `https://novafile.org/file/o70pbnse8zna` |
| `https://dwp.la/d/dro` | `200` after one redirect, sets `affiliate=…; domain=.downup.me` | `https://downup.me/1ms2qj2l3uqr/The_Economist_USA_-_19_September_2026_downmagaz.net.pdf.html` |

Both are affiliate link-cloakers. `dwp.la` is a front of `downup.me` and not merely a
redirector: `https://dwp.la/login.html` redirects to `https://downup.me/`, and the two serve a
byte-identical `robots.txt`.

Both real targets are XFileSharing installations — `downup.me/login.html` and
`novafile.org`'s file page both carry the XFS `name="F1"` form with `op`/`id` inputs — and
**`downup.me` is already in `plugins/xfs-generic/manifest.toml`**. Nothing about the `dwp.la`
chain needs a new resolver; it needs the redirect followed. `novafile.org` is not in that list.

**This was half-known and half-wrong already.** RD-110-10 measured the same `downmagaz.net`
page on its own day and wrote down that `dwp.la/d/<id>` "ist ein Kürzel und antwortet mit 302 auf
`https://downup.me`" — the redirect was seen. In the same list it called `nfile.cc/<id>` "der
freie Hoster", which it is not. That is the asymmetry this record is about: one cloaked address
was recognised as a shortener because its path looked like one, the neighbouring address on the
identical page was taken for a hoster because its path looked like a file id. Only following it
tells them apart.

**All four addresses in these two chains refuse automated access.** `nfile.cc`, `novafile.org`,
`dwp.la` and `downup.me`:

```
# nfile.cc, novafile.org          # dwp.la and downup.me, byte-identical
User-agent: *                     User-agent: *
Disallow: /                       Allow: /$
                                  Allow: /pages/tos … /pages/dmca
                                  Allow: /css/ /js/ /images/
                                  Disallow: /
```

The second form is the sharper one: it allows the static pages and the legal pages by name and
disallows everything else, which is precisely the `/d/<id>` and `/<id>/<name>.html` download
route a resolver would walk.

**ViperGirls' targets, measured for the first time.** The rule's own probe
(`threads/6629687-Adult-Magazines-Mix-Collection`) plus four threads sampled from
`sitemap_forum_1.xml.gz`. `robots.txt` is permissive (`Disallow:` empty). The file hosts found,
with what already claims them:

| Host | Occurrences | Status |
| --- | --- | --- |
| `rapidgator.net` | 31 | covered — `plugins/rapidgator` |
| `filefox.cc` | 30 | covered — `plugins/xfs-generic` (XFS confirmed live) |
| `katfile.com` | 18 | covered — `plugins/katfile` |
| `k2s.cc` | all four sampled threads | covered — `plugins/keep2share` |
| `oxy.cloud` | 12 | **dead domain** |
| `imx.to`, `i116.fastpic.org` | many | image hosts; gallery pipeline, not hosters |
| `r3dbng.com` | 1 | ad iframe, inside an HTML comment |

`oxy.cloud` does not resolve, and not locally: a DNS-over-HTTPS query to Cloudflare returns
`Status=3` (NXDOMAIN) for both `A` and `NS`, with an `SOA` from `ns.trs-dns.com` /
`trs-ops.tucows.com` — the registrar's parking nameservers. The domain is gone at the registry.
Its addresses on ViperGirls have the same `/d/<short>` shape as `dwp.la`, so it was very likely
the same kind of affiliate front.

## Decision

**No new resolver for `nfile.cc`, `dwp.la` or `oxy.cloud`,** and none for the two hosts behind
them. RD-120-19 records all three as measured No-Gos.

Each for its own reason, and none of them is "the hoster is hard":

- `dwp.la` — there is no hoster to add. `downup.me` behind it is already claimed by
  `xfs-generic`. Writing a `dwp.la` resolver would be writing a redirect follower and calling it
  a hoster.
- `nfile.cc` — same shape, and `novafile.org` behind it answers `Disallow: /`, as does
  `nfile.cc` itself. An operator disallowing the whole site is declining automated access to the
  exact route a resolver would walk; that settled two services in wave 0 and settles this one.
- `oxy.cloud` — the domain does not exist. Nothing to measure further.
- ViperGirls needs no hoster work at all. Its four file hosts are already covered by three of
  this application's own resolvers and by `xfs-generic`.

## Consequences

No code, no plugin directory, no provider entry, no manifest change.

The premise RD-120-19 started from turns out to be half right. A rule handing over an address
nobody resolves is real, but for two of the three addresses the missing piece is not a hoster
plugin — the address is a cloaked affiliate link, and the hoster behind it is either already
supported or refusing robots. **A host list built from what a board displays will collect
redirectors.** Resolve the address before concluding a hoster is missing; this record exists so
the next reader does the redirect first and the plugin never.

Whether the collector should follow such a redirect at intake — so a `dwp.la` link becomes the
`downup.me` link `xfs-generic` already claims — is a real question and deliberately not decided
here. It is a collector decision about untrusted redirects, not a hoster decision, and it needs
its own job.

Noted and not acted on, because it belongs to whoever owns that list: `downup.me` is in
`xfs-generic`'s `match_domains` while its `robots.txt` disallows the download path. `filefox.cc`
is in the same list and serves an empty `robots.txt` (zero bytes), which disallows nothing.
