# HitFile fixtures

Measured on 2026-09-21 with `curl -A 'Mozilla/5.0'` from WSL, without an account, without
cookies, without JavaScript, against `https://hitfile.net/` and `https://app.hitfile.net/api`.
Files whose name carries the date are live answers; `*-synthetic.json` were written by hand in
the shape the feasibility measurement of the same day recorded (job file `103-11`, section 2).
`free-start-premium-only-400-feasibility.json` quotes the `400` body that measurement saw for
the premium-only file; the run that produced these fixtures got `409` `Download url not found`
for the same file after `free/init` had answered `directHit: true` — both are kept, and both
end in a structured failure. The login half shares Turbobit's synthetic fixtures.

Sanitised before saving: the three file ids (`Ab1CdEf` premium-only, `Gh2IjKl` free,
`Mn3OpQr` deleted), the two file names, the Sentry DSN and the metrics id in the SPA shell. No
`sid`, `sign`, IP address or cookie was ever written to disk.

Live facts these fixtures pin: the `.html` form of a HitFile link is `invalid` for
`links/check` (`links-check-mixed-*`), `premiumOnlyDownload: true` is public for `Ab1CdEf`, and
the API answers JSON errors only with `Accept: application/json`
(`free-start-without-accept-409-*.html` is what arrives without it).
