# {{PLUGIN_NAME}}

An `intake` plugin scaffolded by `rdownloader plugin new --type intake`. Turns text and URLs the
built-in scanner does not understand into LinkGrabber candidates.

Read [the `intake` section of the plugin
reference](https://github.com/degoya/rDownloader/wiki/plugin-reference#intake-parsers) before
changing it: it describes what the host promises this type and what it refuses.

[Writing an intake parser](https://github.com/degoya/rDownloader/wiki/intake-plugin)
in the handbook walks from this scaffold to a signed package in a repository of your own.

## What is here

- `src/lib.rs` — claiming, reading and normalizing, with tests
- `src/guest.rs` — the three calls of the intake world
- `manifest.toml` — identity, capabilities and limits; `plugin_type = "intake"`
- `locales/en.json` — the translations of every failure code the plugin reports
- `wit/rdownloader.wit` — the contract, a copy of the one rDownloader speaks

It asks for nothing — a parser reads the text it is handed. Ask for exactly what you use: the plugin
manager shows the grants to the person deciding whether to install it.

## Two rules worth knowing first

- You propose, the application decides: everything goes through the LinkGrabber review.
- `normalize` tidies an address; a rewrite that changes host or scheme is discarded.

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
