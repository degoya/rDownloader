# {{PLUGIN_NAME}}

A `transfer` plugin scaffolded by `rdownloader plugin new --type transfer`. Carries the bytes of a
protocol the application does not know, on the same queue and under the same speed limit.

Read [the `transfer` section of the plugin
reference](https://github.com/degoya/rDownloader/wiki/plugin-reference#transfer-backends) before
changing it: it describes what the host promises this type and what it refuses.

[Writing a transfer backend](https://github.com/degoya/rDownloader/wiki/transfer-plugin)
in the handbook walks from this scaffold to a signed package in a repository of your own.

## What is here

- `src/lib.rs` — address, reply line and checkpoint, with tests
- `src/guest.rs` — `probe`, `run`, the connection
- `manifest.toml` — identity, capabilities and limits; `plugin_type = "transfer"`
- `locales/en.json` — the translations of every failure code the plugin reports
- `wit/rdownloader.wit` — the contract, a copy of the one rDownloader speaks

It asks for `net_stream` (the hosts and ports it dials, no wildcard port). Ask for exactly what you
use: the plugin manager shows the grants to the person deciding whether to install it.

## Two rules worth knowing first

- Resume from `sink.committed()`, not from your checkpoint.
- Check `should-stop` between reads, or the execution budget stops you without a checkpoint.

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
