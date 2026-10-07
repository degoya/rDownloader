# {{PLUGIN_NAME}}

A `stream-transform` plugin scaffolded by `rdownloader plugin new --type stream-transform`. Answers
with an address *and* how the bytes behind it become a file — for a provider that encrypts on the
client and keeps the key in the link.

Read [the `stream-transform` section of the plugin
reference](https://github.com/degoya/rDownloader/wiki/plugin-reference#stream-transforms) before
changing it: it describes what the host promises this type and what it refuses.

[Writing a stream transform](https://github.com/degoya/rDownloader/wiki/stream-transform-plugin)
in the handbook walks from this scaffold to a signed package in a repository of your own.

## What is here

- `src/link.rs` — the address and the key in its fragment, with tests
- `src/reply.rs` — reading the provider's answer, with tests
- `src/guest.rs` — `claims-url` and `resolve`
- `manifest.toml` — identity, capabilities and limits; `plugin_type = "stream-transform"`
- `CHANGES.md` — the release notes, a `## <version>` section per version in one to three
  sentences for the people who install it
- `locales/en.json` — the translations of every failure code the plugin reports
- `wit/rdownloader.wit` — the contract, a copy of the one rDownloader speaks

It asks for `net_http` (the provider's API and its storage hosts) and `secret_fragment_domains` (the
hosts whose fragment is the key). Ask for exactly what you use: the plugin manager shows the grants
to the person deciding whether to install it.

## Two rules worth knowing first

- The plugin describes, the host computes: name a primitive the host implements, never compute a
  byte yourself.
- The key never goes into a request — the provider is the one party that must never learn it.

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
