# Writing an rDownloader plugin

A plugin is a signed WebAssembly component. It runs in a sandbox with no file system, no
processes and no WASI, and reaches exactly the interfaces its manifest asks for — nothing is
granted implicitly and nothing can be requested at run time.

There are twelve kinds. Eleven of them have a scaffold, one per `--type`:

| Type | What it does |
| --- | --- |
| `resolver` | Turns a hoster link into a downloadable URL and checks whether links are alive |
| `transfer` | Carries the bytes of a protocol the application does not know |
| `intake` | Turns text and URLs the built-in scanner does not understand into LinkGrabber candidates |
| `auth` | Signs an account in through the provider's own flow, without a password being typed here |
| `oauth` | Signs an account in by OAuth redirect or device code, and renews the token afterwards |
| `crawler` | Turns one address — a cloud folder, a directory share — into the files behind it |
| `enricher` | Adds metadata to a link before it is downloaded |
| `notifier` | Delivers notifications to one more destination |
| `postprocess` | Runs one more step after a package has been downloaded |
| `storage` | Uploads finished packages somewhere |
| `remote-job` | Runs a job that lives at a provider and outlives the call — a magnet at a debrid account |

Each scaffold is a working plugin of its type, with the promises the host makes to that type
written at the top of `src/lib.rs` — they are what shapes the code, so they are worth reading
before changing it.

The twelfth kind, `stream-transform`, has **no scaffold yet**. It answers with an address *and*
a declarative description of how the bytes behind it become a file — the world for a provider
that encrypts on the client and keeps the key out of its own reach (RD-110-33, ADR 0011). The
contract is already in every template's `wit/rdownloader.wit`, so writing one means scaffolding
any other type, setting `plugin_type = "stream-transform"` in `manifest.toml` and pointing
`[package.metadata.component.target] world` at `stream-transform-plugin`.
`docs/plugins.md` describes the world, the two primitives the host implements and the four
properties an author has to write against; `plugins/example-stream-transform/` in the
repository is a working reference.

## Start

```bash
rdownloader plugin new --type resolver --out ./myhoster
cd myhoster
cargo component build --release --target wasm32-unknown-unknown
```

`plugin new` reads the templates from `sdk/templates`, so run it from a checkout of this
repository or from a directory that holds a copy of that folder; a release binary on its own
does not carry them. The directory name becomes the crate name and the manifest's provider
slug; `--name` sets a different one. The scaffold needs `cargo-component` 0.21.1 — the version
the bundled plugins are built with — and the `wasm32-unknown-unknown` target, which
`rustup target add` installs.

The scaffold compiles, packages and passes conformance before you change a line of it — so the
first failure you see is about your code and not about the setup. It brings its own copy of the
WIT contract in `wit/`, so nothing about the build needs a checkout of rDownloader, and its own
Ed25519 key pair in `plugin-signing.key`, which belongs in your secret store and not in version
control.

```bash
rdownloader plugin package --manifest manifest.toml \
  --component target/wasm32-unknown-unknown/release/myhoster.wasm \
  --locales locales --key plugin-signing.key --output myhoster.rdplug
rdownloader plugin conformance myhoster.rdplug --json
```

`--key` may be left out when `RDOWNLOADER_PLUGIN_SIGNING_KEY` holds the PEM text, which is how
the CI workflow below signs. `--development` writes an unsigned package instead; only
`serve --plugin-development-mode` and `verify --development-mode` accept one, so it is for a
local test and never for a release.

The `oauth` scaffold is the one with logic of its own: a full authorization-code flow with
PKCE **and** a device-code flow beside it, including SHA-256 and base64url written out in `src/pkce.rs`, because a guest has no WASI
to borrow them from. The PKCE verifier and the `state` are built from `host.random-bytes` — 32
bytes of operating-system entropy each, drawn separately — and from nothing else. A value
computed from the clock and the account id, which an earlier version of this scaffold used, is
one anybody holding those public inputs can recompute, and a recomputable verifier defeats the
whole point of PKCE. An empty answer from `host.random-bytes` is a refusal: fail the sign-in on
it. Everything that does not need a WebAssembly toolchain lives outside the component, so
`cargo test` in a fresh scaffold runs its unit tests straight away:

```bash
rdownloader plugin new --type oauth --out ./myprovider
cd myprovider && cargo test
```

It ships serving both ways in — `oauth_flows = ["redirect", "device"]` at the top of
`manifest.toml`, above the first `[table]` header, or TOML makes it a key of that table. Delete
the entrance your provider does not offer, from the manifest and from `src/guest.rs` alike: the
host calls only what the manifest names, so there is no stub to keep working. A manifest that
leaves the field out serves the redirect and nothing else.

Read `docs/plugins.md`, section *OAuth providers*, before changing it — especially the three
rules the flow turns on: a provider that refuses ends the sign-in while one that cannot be
reached must not, "not confirmed yet" is waiting rather than either, and nothing a provider
wrote is ever repeated verbatim.

The `crawler` scaffold is the other one that arrives with working logic: a breadth-first walk
of a folder tree with its own limits on depth, breadth and cycles, an address matcher that
claims narrowly, and a listing reader — none of which needs a WebAssembly toolchain, so
`cargo test` runs twelve unit tests in a fresh scaffold here too:

```bash
rdownloader plugin new --type crawler --out ./myfolders
cd myfolders && cargo test
```

Read `docs/plugins.md`, section *Folder crawlers*, and
`docs/adr/0001-resolving-an-address-that-points-at-many-files.md` before changing it. The two
rules that matter most: the walk bounds itself rather than leaning on the fuel budget, and an
empty or unreachable folder is a failure with a stable code — never an empty list, which would
create a package with nothing in it.

The `remote-job` scaffold is the third with logic of its own, and the one to read if you are
writing for a debrid provider. A remote job is work that happens at somebody else's provider and
outlives the call that started it: a magnet handed over answers with an identifier, not a file,
runs for minutes or hours, stops half-way until a person has said which files they want, and
stays in the account afterwards. Seven short calls carry it — `claims`, `identify`, `submit`,
`adopt`, `poll`, `choose`, `discard` — and **none of them waits**. The row, the remote
identifier, the clock, the person's answer and the restart belong to the host. Ask
`awaiting-choice` only if your provider keeps the answer, because the poll after `choose` has to
read the answer back from the provider. The host never hands it to the guest again, and a
question asked a second time ends the job under `remote_job.choice_not_kept` (RD-120-35).
`cargo test` runs twelve unit tests in a fresh scaffold here too:

```bash
rdownloader plugin new --type remote-job --out ./myprovider
cd myprovider && cargo test
```

Read `docs/plugins.md`, section *Remote jobs*, and
`docs/adr/0003-a-job-that-runs-at-the-provider.md` before changing it. The two rules that matter
most: `identify` never makes a request and never invents a key — it is derived locally, the host
writes it down before it asks the provider for anything, and a unique index on it is what makes
a duplicate submit impossible at a provider whose submit is not idempotent; and `discard` is
reached from one explicit, confirmed request and never as a side effect, because what rDownloader
did not put in somebody's account on its own it does not take out on its own.

The scaffold derives that key the way BitTorrent does — the info hash of a magnet or of a
`.torrent` file's `info` dictionary, in `src/source.rs`, with SHA-1 written out in
`src/digest.rs` so the only dependency stays `wit-bindgen`. If your provider keys its jobs by
something else, replace those two modules and keep the two properties: derived without a
request, and the same for the same content every time.

## Translations

The scaffold ships `locales/en.json`, and English is the one language a localised plugin must
carry: a package with any locale file and no `en.json` is refused. The web interface is offered
in German, English, Spanish and French, and the bundled plugins ship all four as `de.json`,
`en.json`, `es.json` and `fr.json`; a language the plugin lacks falls back to English. The keys
and what they name are in `docs/plugins.md`, section *Translations*.

The label next to a provider account travels the same way as a failure: `account-status.label`
is a list of `label-part { code, params, message }`, and the interface translates each part —
active language, then English, then the `message` text — and joins them. The scaffold answers
with `plugin.account.premium_unchecked`, one of the core codes every installation translates
(`plugin.account.user`, `premium_until`, `premium_lifetime`, `premium_expired`, `cookies`,
`signed_in`, `session_active`); anything only your provider says gets a code in your own
`<slug>.` namespace and a line in each `locales/*.json`. A part without a code is refused by
the host. The full table is in `docs/plugins.md`, section *The account label*.

## What conformance checks

`plugin conformance` answers one question: would this core run your package? It verifies the
archive, the manifest and the signature exactly as installation does, confirms the ABI version
is one the core supports, instantiates the component against the world its type declares, and
— for a resolver — asks the component itself whether it claims the links its manifest says it
claims, and whether it wrongly claims a link belonging to nobody.

It does **not** check that your plugin resolves a real link. That needs a hoster, an account and
a network, none of which belong in a conformance run. A package that passes every check can
still be wrong about its provider; the report says what was verified, not that the plugin is
good.

`--json` prints a report with one entry per check, and the command exits non-zero if anything
failed, so a CI job can gate on it without parsing output.

## Capabilities

Each grant in `[capabilities]` is one WIT interface. A component that imports an interface its
manifest does not grant is refused when it is packaged and again when it is installed, and it
cannot be instantiated at all — so asking for less than you use fails loudly at build time
rather than quietly at a user's download. Ask for exactly what you use: the plugin manager shows
your grants to the person deciding whether to install you.

The file system, other processes and the network beyond your declared domains are not
capabilities that happen to be denied. There is no interface for them.

A component that imports anything outside `rdownloader:plugin` — above all a `wasi:` interface
pulled in by a dependency — is refused the same way. `scripts/check-plugin-imports.sh` in this
repository reads the import section with `wasm-tools` and says which interface it was; the CI
workflow below runs the same check.

## Captchas

The `captcha` grant hands a challenge to the application, which either buys an answer from a
configured solver service or asks the person. Six kinds exist, and the shape of the answer
decides which call to make:

- `recaptcha-v2`, `hcaptcha`, `turnstile` — a `widget-challenge` with site key and page.
  Answered by a solver service or by the person in their own browser through the extension.
  The answer is a token.
- `image` — an `image-challenge`: the raw picture, its MIME type and an optional prompt.
  Answered by a service or typed by the person in the web interface. The answer is the text.
- `click-point` — the same `image-challenge`, but the answer is a spot in the picture:
  `click-point { x, y }` in pixels of the image as you served it. The person clicks it in the
  web interface; a service solves it as a coordinate task.
- `cutcaptcha` — a `cutcaptcha-challenge` with the widget's `site-key` (its `data-apikey`),
  the page's `misery-key` (`CUTCAPTCHA_MISERY_KEY`) and the page. Only a solver service can
  answer one; without a configured service the call ends at once with
  `captcha.cutcaptcha_needs_solver`.

`solve-captcha` returns `captcha-solution { token }` and refuses a `click-point` challenge with
`captcha.answer_shape` before anyone is asked. `solve-challenge` takes every kind and returns
`captcha-answer`, a `token` or a `point`. In `plugin-common` the same two calls are
`PluginHost::solve_captcha` and `PluginHost::solve_challenge`. Every widget page, CutCaptcha
included, must lie inside your declared domains. The full table is in `docs/plugins.md`.

## CI

`ci/plugin.yml` is a reusable GitHub Actions workflow that builds, checks for accidental WASI
imports, packages, verifies and runs conformance. Copy it into `.github/workflows/`, set
`PLUGIN_SIGNING_KEY` as a repository secret holding the PEM text of `plugin-signing.key`, and
put your crate name into `COMPONENT`. It downloads the rDownloader release named in
`RDOWNLOADER_VERSION` for `plugin package`, `verify` and `conformance` — the binary embeds the
built web interface, so it cannot be `cargo install`ed from the repository — and that version
is the one you bump when you build against a newer contract.

## The contract

`wit/rdownloader.wit` in your scaffold is the contract, and `docs/plugins.md` in the rDownloader
repository explains the manifest field by field. Both are versioned: `api_version` in your
manifest names the WIT package version you built against, and a core that does not support it
refuses your package rather than running it against a contract you did not write for.

The current package is `rdownloader:plugin@0.9.0` (RD-130-11). It adds two functions to
`interface remote-job`: `cache-kinds`, which names the kinds of source your provider's cache can
be asked about (`torrent`, `usenet`, `hoster`), and `check-cached`, which answers for a batch of
sources -- one answer per query, in order, read-only at the provider, `cached`, `known` or
`unknown` and never `offline`. The `remote-job` template answers "no kinds" and `unknown`, which
is right for every provider without a cache query; `docs/plugins.md` ("The cache question")
has the rules. A plugin of any other world builds unchanged against the new `wit/`, but it has
to be rebuilt: a plugin built against `0.8.0` does not load any more. It is refused as
`plugin.capability_unknown` (unknown `api_version`) and stays listed in the plugin manager.
Rebuild it against the new `wit/` and raise `api_version` together with your plugin's `version`.

`0.8.0` (RD-120-36) added one case to `link-status`: `cached`, for a file your provider says it
holds in its own cache right now. That is not the same as `online`. The host shows it with the
time of the check and never as a promise, so answer it only when the provider actually said
"cached".
