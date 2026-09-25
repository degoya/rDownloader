# Turbobit fixtures

Measured on 2026-09-21 with `curl -A 'Mozilla/5.0'` from WSL, without an account, without
cookies, without JavaScript, against `https://turbobit.net/` and `https://app.turbobit.net/api`.
Files whose name carries the date are live answers; files named `*-synthetic.json` were written
by hand in the shape the feasibility measurement of the same day recorded (job file `103-10`,
section 2) or, for everything behind the login, in the shape JDownloader's `TurbobitCore`
(r52763) reads — nothing behind the login was measured.

Sanitised before saving: the file id (`a1b2c3d4e5f6`), the file name (`Sample File 1.pdf`, the
space is kept on purpose), the Sentry DSN and the metrics id in the SPA shell. No `sid`,
`sign`, IP address or cookie was ever written to disk; the direct-link answers are synthetic
for that reason.

Two answers that shaped the code:

- The API answers a validation failure as JSON (`422`) only when the request carries
  `Accept: application/json`; without it the same call answers `302` with the HTML page in
  `free-captcha-without-accept-302-*.html`. The resolver sends the header on every call.
- `free/init` for the live file answered `{"directHit":true}` because the guest window opened
  by the morning's measurement was still running — this is the host-block case, measured live.
  The length of that window is not measured; the code assumes 60 minutes and says so.
