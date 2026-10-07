# {{PLUGIN_NAME}}

An `oauth` plugin scaffolded by `rdownloader plugin new --type oauth`. Signs an account in by OAuth
redirect with PKCE or by device code, and renews the token afterwards.

Read [the `oauth` section of the plugin
reference](https://github.com/degoya/rDownloader/wiki/plugin-reference#oauth-providers) before
changing it: it describes what the host promises this type and what it refuses.

[Writing an OAuth sign-in plugin](https://github.com/degoya/rDownloader/wiki/oauth-plugin)
in the handbook walks from this scaffold to a signed package in a repository of your own.

## What is here

- `src/pkce.rs` — PKCE, SHA-256 and base64url written out, with tests
- `src/flow.rs` — reading token answers, with tests
- `src/guest.rs` — both entrances and `refresh`
- `manifest.toml` — identity, capabilities and limits; `plugin_type = "oauth"`
- `CHANGES.md` — the release notes, a `## <version>` section per version in one to three
  sentences for the people who install it
- `locales/en.json` — the translations of every failure code the plugin reports
- `wit/rdownloader.wit` — the contract, a copy of the one rDownloader speaks

It asks for `net_http` (the provider's authorization and token endpoints). Ask for exactly what you
use: the plugin manager shows the grants to the person deciding whether to install it.

## Two rules worth knowing first

- PKCE verifier and `state` come from `host.random-bytes` and nothing else.
- Delete the entrance your provider does not offer from `oauth_flows` and from `src/guest.rs`.

## Build, test, package

```bash
cargo test
cargo build --release --target wasm32-unknown-unknown
wasm-tools component new target/wasm32-unknown-unknown/release/{{PLUGIN_SLUG}}.wasm \
  -o target/{{PLUGIN_SLUG}}.wasm
rdownloader plugin package --manifest manifest.toml \
  --component target/{{PLUGIN_SLUG}}.wasm \
  --locales locales --key plugin-signing.key --output {{PLUGIN_SLUG}}.rdplug
rdownloader plugin conformance {{PLUGIN_SLUG}}.rdplug --json
```

`cargo test` runs the unit tests on your own machine, without a WebAssembly toolchain.
`cargo build` makes a WebAssembly core module that carries the world `src/guest.rs` generates, and
`wasm-tools component new` turns it into the component — no WASI adapter, the plugin imports
nothing but the contract. Use the wasm-tools version the SDK's `ci/plugin.yml` installs, the one
rDownloader's own plugins are made with.
`plugin-signing.key` is yours alone: keep it out of version control. Signing, the reusable CI
workflow and publishing in a repository of your own are in the SDK's `README.md` and the [plugin
reference](https://github.com/degoya/rDownloader/wiki/plugin-reference).
