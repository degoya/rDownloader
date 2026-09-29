# {{PLUGIN_NAME}}

A `remote-job` plugin scaffolded by `rdownloader plugin new --type remote-job`. Runs a job that
lives at a provider and outlives the call — a magnet at a debrid account.

Read [the `remote-job` section of the plugin
reference](https://github.com/degoya/rDownloader/wiki/plugin-reference#remote-jobs) before changing
it: it describes what the host promises this type and what it refuses.

[Writing a remote job plugin](https://github.com/degoya/rDownloader/wiki/remote-job-plugin)
in the handbook walks from this scaffold to a signed package in a repository of your own.

## What is here

- `src/source.rs` — the content key of a magnet or `.torrent`
- `src/digest.rs` — SHA-1 written out
- `src/reply.rs` — reading the provider's answers
- `src/guest.rs` — the seven calls
- `manifest.toml` — identity, capabilities and limits; `plugin_type = "remote-job"`
- `locales/en.json` — the translations of every failure code the plugin reports
- `wit/rdownloader.wit` — the contract, a copy of the one rDownloader speaks

It asks for `secrets` (the account's token slot) and `net_http` (the provider's API). Ask for
exactly what you use: the plugin manager shows the grants to the person deciding whether to install
it.

## Two rules worth knowing first

- `identify` never makes a request and never invents a key.
- `discard` is reached from one explicit, confirmed request only.

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
