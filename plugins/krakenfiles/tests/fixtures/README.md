# KrakenFiles fixtures

Measured on 2026-09-21 between 07:43 and 07:47 UTC from WSL with `curl`, User-Agent
`Mozilla/5.0 (X11; Linux x86_64; rv:128.0) Gecko/20100101 Firefox/128.0`, without an
account, without solving a captcha and without downloading a file (RD-103-08, phase 2). The
file is the public example the feasibility measurement used, `DP3nGKJNsX`
(`EldenRing_Fix_Repair_Steam_Generic.rar`, 4.90 MB, uploaded 13-08-2022).

| File | Request | Answer |
| --- | --- | --- |
| `file-page-2026-09-21.html` | `GET /view/DP3nGKJNsX/file.html` | 200, 45 052 B: `<form action="/download/DP3nGKJNsX" id="dl-form">` with the hidden `token`, empty `userdata` and `fingerprint`, `data-file-hash`, the Turnstile widget |
| `json-file-2026-09-21.json` | `GET /json/DP3nGKJNsX` | 200, the metadata object; `/json/dp3ngkjnsx` and `/json/DP3NGKJNSX` answer the same object, so the id is case-insensitive here too |
| `json-missing-2026-09-21.json` | `GET /json/zTpdkgdZY8` (deleted) | 200, `[]` |
| `error-page-2026-09-21.html` | `GET /view/doesnotexist0/file.html` | 404, "File has been deleted or never existed" |
| `download-captcha-invalid-2026-09-21.json` | `POST /download/DP3nGKJNsX` with the page's `token`, header `hash`, no Turnstile answer | 500 `application/json`; the same answer with `cf-turnstile-response=invalid` |
| `embed-video-2026-09-21.html` | `GET /embed-video/DP3nGKJNsX` | 200, the site's own player; streams from `hs3.krakencloud.net` |
| `download-ok-synthetic.json` | — | **Synthetic.** The successful answer needs a solved Turnstile and was not measured; the shape (`status: "ok"`, `url`) follows JDownloader's `KrakenfilesCom` and pyLoad's `KrakenfilesCom`, the host is a guess inside the manifest's `download_domains`. |

Sanitised: the form's `token` value, the ad hash, the Cloudflare challenge parameters and
obfuscated e-mail, the analytics id, the ad-network keys, the site-verification metas, and
the embed page's ping and play tokens are replaced by placeholders. The Turnstile and
reCAPTCHA site keys are public page content and are kept, because the parser reads them.
