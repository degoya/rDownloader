# {{PLUGIN_NAME}}

A `postprocess` plugin scaffolded by `rdownloader plugin new --type postprocess`. Runs one more step
after a package has been downloaded — here a CRC-32 per file.

Read [the `postprocess` section of the plugin
reference](https://github.com/degoya/rDownloader/wiki/plugin-reference#post-processing-steps) before
changing it: it describes what the host promises this type and what it refuses.

[Writing a post-processing step](https://github.com/degoya/rDownloader/wiki/postprocess-plugin)
in the handbook walks from this scaffold to a signed package in a repository of your own.

## What is here

- `src/lib.rs` — the checksum and the checkpoint, with tests
- `src/guest.rs` — `run`
- `manifest.toml` — identity, capabilities and limits; `plugin_type = "postprocess"`
- `CHANGES.md` — the release notes, a `## <version>` section per version in one to three
  sentences for the people who install it
- `locales/en.json` — the translations of every failure code the plugin reports
- `wit/rdownloader.wit` — the contract, a copy of the one rDownloader speaks

It asks for nothing — reading the package is part of the type, through a handle. Ask for exactly
what you use: the plugin manager shows the grants to the person deciding whether to install it.

## Three rules worth knowing first

- Nothing to do is `skipped`, not `failed`.
- The checkpoint holds everything a resumed run needs, not only an offset.
- A pass with something to say says it in `warnings` on `complete`, translated from `locales/`;
  `failed` is for a package that is wrong. Files earlier steps removed are in `input.removed`.

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
