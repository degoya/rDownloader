# {{PLUGIN_NAME}}

An `enricher` plugin scaffolded by `rdownloader plugin new --type enricher`. Adds metadata to a link
before it is downloaded — here what a release name says.

Read [the `enricher` section of the plugin
reference](https://github.com/degoya/rDownloader/wiki/plugin-reference#metadata-enrichers) before
changing it: it describes what the host promises this type and what it refuses.

[Writing a metadata enricher](https://github.com/degoya/rDownloader/wiki/enricher-plugin)
in the handbook walks from this scaffold to a signed package in a repository of your own.

## What is here

- `src/lib.rs` — reading the name, with tests
- `src/guest.rs` — `enrich`
- `manifest.toml` — identity, capabilities and limits; `plugin_type = "enricher"`
- `locales/en.json` — the translations of every failure code the plugin reports
- `wit/rdownloader.wit` — the contract, a copy of the one rDownloader speaks

It asks for nothing — add `net_http` only if you look something up. Ask for exactly what you use:
the plugin manager shows the grants to the person deciding whether to install it.

## Two rules worth knowing first

- Your fields are added, never substituted; prefix them with your slug.
- An empty list means nothing to add, not a failure.

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
