# Security Policy

rDownloader is a download manager that people run on their own machines, usually with stored
hoster credentials, Usenet passwords and a queue that reaches out to the open internet. A
vulnerability here is somebody's server, so reports are welcome and taken seriously.

## Reporting a vulnerability

**Report it privately on GitHub** at
`https://github.com/degoya/rDownloader/security/advisories/new` (the repository's **Security** tab,
then **Report a vulnerability**). A private vulnerability report is visible only to the
maintainers, which makes it the right channel for something that should not be public yet.

Please do not open a public issue, and please do not post a working exploit anywhere public,
until a fixed release exists.

Useful in a report, roughly in order of how much they help:

- What an attacker gains, and what they need first — network reach, a session, a token, a
  crafted link, a plugin.
- The version (`rdownloader --version`), the platform, and whether it runs behind a reverse
  proxy or with a non-empty base path.
- The smallest reproduction you have. A `curl` command beats a description.
- Whether the affected surface was reachable from outside the machine.

You will get an acknowledgement within a week. This is a small project and there is no
on-call rotation, so please read that as a realistic figure rather than a service level.

## What is in scope

Everything shipped from this repository: the service and its REST, SSE and MCP surfaces, the
desktop capture agent, the browser extension, the plugin host and its trust model, the
compatibility APIs, the container images, and the release artifacts and their signatures.

Some things that look like findings are documented behaviour, and a report about them is
answered with a link rather than a fix:

- **The default deployment binds to loopback and has no password until setup runs.** That is
  the intended first-run state on a machine you already control.
- **The administrator can run arbitrary post-processing scripts and install plugins.** These
  are features. The boundary being defended is *between* the plugin sandbox and the host, and
  between an unauthenticated caller and the service — not between the administrator and their
  own machine.
- **A token holding `api:admin` or `api:secrets` can do administrative or credential things.**
  That is what those areas mean. A path that reaches them *without* the matching area is a
  finding, and a serious one.
- **Rate limiting on the login is deliberately incapable of locking the account out.** The
  per-address lockout is real; the global limiter only delays. On a single-account service an
  attacker who could trigger a lockout would have a denial of service against the owner.

## Supported versions

Security fixes go into the most recent release. There are no long-term-support branches and no
backports to older lines — with one maintainer, a backport branch that is not actually tested is
worse than an honest "upgrade".

| Version | Security fixes |
| --- | --- |
| Latest release | Yes |
| Anything older | No — upgrade |

Because updates are not yet delivered in-app, staying current means watching the releases page.
That is a known gap, and the first public release — the one that closes milestone 1.6 — keeps it:
the in-app updater with signed manifests and channels is planned for milestone 1.8, together with
the installers and platform signing.

## What the project already does

Stated so a report can be aimed at what is *not* covered, rather than at what is:

- Administrator passwords are hashed with Argon2id; session cookies are `HttpOnly` and
  `SameSite=Strict`, and sessions are stored as a SHA-256 digest rather than as the bearer.
- Optional two-factor sign-in (TOTP) and passkeys, the latter bound to the configured external
  URL so the credential cannot be phished onto another origin.
- Machine tokens carry one or more of six permission areas; nothing confers stored credentials
  or administration implicitly, and the route policy is checked against the OpenAPI document in
  both directions by a test.
- Stored credentials live in an encrypted store behind references, never in the database in
  clear, and are excluded from settings backups unless explicitly exported with a passphrase.
- Plugins are WebAssembly components with no WASI access, verified against an Ed25519 trust
  root, limited by fuel, memory and an allowed-domain list.
- Release artifacts ship an SPDX SBOM, a SHA-256 manifest and a Sigstore bundle; container
  images carry provenance and a Cosign signature.

## Credit

Reporters are credited in the changelog entry for the fix unless they ask not to be. There is no
bug bounty — this is an unpaid project, and pretending otherwise would waste your time.
