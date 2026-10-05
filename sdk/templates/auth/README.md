# {{PLUGIN_NAME}}

An `auth` plugin scaffolded by `rdownloader plugin new --type auth`. Signs an account in through the
provider's own flow — here a PIN — without a password being typed into rDownloader.

Read [the `auth` section of the plugin
reference](https://github.com/degoya/rDownloader/wiki/plugin-reference#authentication-providers)
before changing it: it describes what the host promises this type and what it refuses.

[Writing a sign-in plugin](https://github.com/degoya/rDownloader/wiki/auth-plugin)
in the handbook walks from this scaffold to a signed package in a repository of your own.

## What is here

- `src/lib.rs` — reading the provider's answers and the flow state, with tests
- `src/guest.rs` — `begin` and `poll`
- `manifest.toml` — identity, capabilities and limits; `plugin_type = "auth"`
- `locales/en.json` — the translations of every failure code the plugin reports
- `wit/rdownloader.wit` — the contract, a copy of the one rDownloader speaks

It asks for `net_http` (the API and the page the person confirms on). Ask for exactly what you use:
the plugin manager shows the grants to the person deciding whether to install it.

## Two rules worth knowing first

- Store the key with `credentials.store-token` before answering `authorized`.
- `flow-state` is bookkeeping, never a credential.

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
