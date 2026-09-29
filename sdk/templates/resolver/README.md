# {{PLUGIN_NAME}}

A `resolver` plugin scaffolded by `rdownloader plugin new --type resolver`. Turns a hoster link into
a downloadable address, checks whether links are alive and reports on the account.

Read [the `resolver` section of the plugin
reference](https://github.com/degoya/rDownloader/wiki/plugin-reference#resolvers) before changing
it: it describes what the host promises this type and what it refuses.

[Writing a resolver plugin](https://github.com/degoya/rDownloader/wiki/resolver-plugin)
in the handbook walks from this scaffold to a signed package in a repository of your own.

## What is here

- `src/lib.rs` — which links are yours, with tests
- `src/guest.rs` — account check, `resolve`, `check`, `hosters`
- `manifest.toml` — identity, capabilities and limits; `plugin_type = "resolver"`
- `locales/en.json` — the translations of every failure code the plugin reports
- `wit/rdownloader.wit` — the contract, a copy of the one rDownloader speaks

It asks for `secrets` (the account's password slot) and `net_http` (the hoster's domains). Ask for
exactly what you use: the plugin manager shows the grants to the person deciding whether to install
it.

## Two rules worth knowing first

- Answer `premium: true` only where the check read a subscription.
- Claim narrowly: a resolver that claims links belonging to nobody fails conformance.

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
