# PEEPLink and AlfaLink entry-page fixtures

Recorded on **2026-09-21** from this machine with

```bash
UA='Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36'
curl -sS -L -A "$UA" <url>
```

— no account, no session taken from a browser, no cookie jar, nothing solved and no hurdle
worked around. These are the same four entry pages, two `404` pages and one deleted entry that
the measurement in `docs/roadmap/jobs/110-17-die-uebrigen-protektoren.md` reports, fetched again
so the bytes themselves are in the tree. Every identifier here comes from a public source (the
URL patterns of the JDownloader plugin mirror and the CDX index of the Internet Archive).

| File | What it is | Recorded size | Foreign links in `<article>` |
| --- | --- | --- | --- |
| `peeplink-0004ae96cef6.html` | `https://peeplink.in/0004ae96cef6`, `200` | 11.155 B | 24 |
| `peeplink-00013b965394.html` | `https://peeplink.in/00013b965394`, `200` | 6.659 B | 1 |
| `peeplink-unknown-404.html` | `https://peeplink.in/aaaaaaaa`, `404` | 4.606 B | 0 |
| `peeplink-deleted-redirect.html` | `https://peeplink.in/0005738dc976`, `200` after a redirect to `https://peeplink.in/` | 8.172 B | 0 |
| `alfalink-02489255ba1048ae9d1328.html` | `https://alfalink.to/02489255ba1048ae9d1328`, `200` | 6.768 B | 9 |
| `alfalink-13e2cd9a35efcd6c6e4766.html` | `https://alfalink.to/13e2cd9a35efcd6c6e4766`, `200` | 6.993 B | 14 |
| `alfalink-unknown-404.html` | `https://alfalink.to/aaaaaaaa`, `404` | 5.474 B | 0 |

## Sanitising

The two `peeplink.in` entry pages carry one line of third-party pop-under advertising loader:
obfuscated JavaScript whose site id, base64 payloads and timestamp are **different on every
request**, which is the only thing that differs between two fetches of the same entry. That one
line is replaced by a comment naming what it was, so the fixture is deterministic and the tree
carries no advertising payload. The recorded sizes above are the sizes before that replacement;
the files are correspondingly shorter. Nothing else is altered.

Nothing user-specific was present to remove: no cookie, no session id, no address, no e-mail,
no token. The `data-sitekey` values of reCAPTCHA and hCaptcha and the `QapTcha` block are left
where they are on purpose — they are the evidence for the finding that those markers belong to
the **login and register popups** and not to the resolution path, and they are public keys of
the site, printed in the job file already.

## What is not here

**There is no fixture for a password-protected entry.** `PrrpLinkIn.java` recognises one by
`value="Enter Access Password"`, and none of the pages above carries it; no public identifier of
a protected entry was findable on either measuring day. The password branch in `src/entry.rs` and
`src/guest.rs` is therefore built and **not tested**, and it stays that way until a real
protected page can be recorded. It is deliberately not replaced by an invented page.
