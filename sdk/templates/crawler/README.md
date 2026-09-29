# {{PLUGIN_NAME}}

A `crawler` plugin scaffolded by `rdownloader plugin new --type crawler`. Turns one address — a
cloud folder, a directory share — into the files behind it.

Read [the `crawler` section of the plugin
reference](https://github.com/degoya/rDownloader/wiki/plugin-reference#folder-crawlers) before
changing it: it describes what the host promises this type and what it refuses.

[Writing a folder crawler](https://github.com/degoya/rDownloader/wiki/crawler-plugin)
in the handbook walks from this scaffold to a signed package in a repository of your own.

## What is here

- `src/target.rs` — which addresses are yours
- `src/listing.rs` — reading a listing
- `src/walk.rs` — the bounded walk
- `src/guest.rs` — `claims-url` and `crawl`
- `manifest.toml` — identity, capabilities and limits; `plugin_type = "crawler"`
- `locales/en.json` — the translations of every failure code the plugin reports
- `wit/rdownloader.wit` — the contract, a copy of the one rDownloader speaks

It asks for `net_http` (the provider's API). Ask for exactly what you use: the plugin manager shows
the grants to the person deciding whether to install it.

## Two rules worth knowing first

- The walk bounds itself on depth, breadth and cycles; the fuel budget is not the bound.
- An empty or unreachable folder is a failure with a stable code, never an empty list.

## Build, test, package

```bash
cargo test
cargo component build --release --target wasm32-unknown-unknown
rdownloader plugin package --manifest manifest.toml \
  --component target/wasm32-unknown-unknown/release/{{PLUGIN_SLUG}}.wasm \
  --locales locales --key plugin-signing.key --output {{PLUGIN_SLUG}}.rdplug
rdownloader plugin conformance {{PLUGIN_SLUG}}.rdplug --json
```

`cargo test` runs the unit tests on your own machine, without a WebAssembly toolchain.
`plugin-signing.key` is yours alone: keep it out of version control. Signing, the reusable CI
workflow and publishing in a repository of your own are in the SDK's `README.md` and the [plugin
reference](https://github.com/degoya/rDownloader/wiki/plugin-reference).
