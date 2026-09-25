# MediaFire fixtures

Captured on 2026-09-21 between 07:40 and 07:50 UTC from this machine with
`curl -A 'Mozilla/5.0'`, no account, no cookies; the file is the public `test-10mb.bin`
(`ipnyzofjcwri357`) from the README of `Gann4Life/mediafiredl`. A file name carrying the date
is a capture; one without is **synthetic** and says so in its first lines.

Sanitised before they were saved: the token in the direct link (`redacted-token`), the
`dkey` of the repair link, the hidden `security` value of the download form, the
Google-Translate customisation id, every `<script>` body, every `<iframe>` and `<noscript>`,
every HTML comment, and the `owner_name` of the API answers (`Redacted Owner`). The quick
keys, file names, sizes and content hashes are the public share's own and stay.

| File | What it is |
| --- | --- |
| `file-page-2026-09-21.html` | the file page, `200`, `downloadButton` present |
| `api-file-get-info-2026-09-21.json` | `file/get_info`, one key, `200` |
| `api-file-get-info-batch-2026-09-21.json` | two keys, one of them deleted: `file_infos[]` plus `skipped` |
| `api-file-get-info-invalid-2026-09-21.json` | the documentation's deleted example key: error 110, HTTP `404` (it was `400` at 00:30 the same day) |
| `api-file-get-info-missing-2026-09-21.json` | a key of the wrong length: error 111, HTTP `400` |
| `api-file-get-links-2026-09-21.json` | `file/get_links` without a token: `normal_download` only |
| `api-file-get-links-direct-denied-2026-09-21.json` | `link_type=direct_download` without a token: error 45 |
| `error-redirect-errno-320-2026-09-21.txt` | the `302` target of a file page that does not exist |
| `file-page-captcha-recaptcha.html` | synthetic |
| `file-page-captcha-checkbox.html` | synthetic |
| `file-page-threshold.html` | synthetic |
| `file-page-password.html` | synthetic |
| `file-page-malware.html` | synthetic |
| `api-error-261.json` | synthetic |
| `error-redirect-errno-999.txt` | synthetic |
