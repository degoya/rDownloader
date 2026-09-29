# {{PLUGIN_NAME}}

An `intake-mirrors` plugin scaffolded by `rdownloader plugin new --type intake-mirrors`. An intake
parser that also states every source of a file — ranked mirrors and hashes — so one transfer can
fetch from several at once.

Read [the `intake-mirrors` section of the plugin
reference](https://github.com/degoya/rDownloader/wiki/plugin-reference#intake-parsers) before
changing it: it describes what the host promises this type and what it refuses.

[Writing an intake parser with mirrors](https://github.com/degoya/rDownloader/wiki/intake-mirrors-plugin)
in the handbook walks from this scaffold to a signed package in a repository of your own.

## What is here

- `src/list.rs` — the mirror-list reader, with tests
- `src/guest.rs` — `parse` for the LinkGrabber, `sets` for the transfer
- `manifest.toml` — identity, capabilities and limits; `plugin_type = "intake"`
- `locales/en.json` — the translations of every failure code the plugin reports
- `wit/rdownloader.wit` — the contract, a copy of the one rDownloader speaks

It asks for nothing — the list carries every address it needs. Ask for exactly what you use: the
plugin manager shows the grants to the person deciding whether to install it.

## Two rules worth knowing first

- `plugin_type` stays `intake`; the world `intake-mirrors-plugin` in `Cargo.toml` is what adds
  `mirror-sets`.
- A set is matched to its candidate by `primary-url`; state only what the document said.

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
