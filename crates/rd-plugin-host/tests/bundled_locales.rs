//! Keeps every bundled plugin's shipped translations consistent with its manifest and code.
//!
//! A plugin owns its strings, so nothing in the core would notice a code that was renamed in
//! `messages.rs` but not in `locales/en.json` — the UI would silently fall back to the raw
//! English text from the backend. These tests fail instead.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

/// Bundled plugin directories, alongside `crates/`.
fn plugin_directories() -> Vec<PathBuf> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../plugins")
        .canonicalize()
        .expect("plugins directory");
    let mut directories: Vec<PathBuf> = std::fs::read_dir(root)
        .expect("read plugins")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.join("manifest.toml").is_file())
        .collect();
    directories.sort();
    // Counted, not reasoned about, and re-measured on 2026-09-23 with
    // `grep -h '^plugin_type' plugins/*/manifest.toml | sort | uniq -c`: twenty-seven resolvers,
    // twelve folder crawlers, eight `oauth` sign-ins, five `auth` sign-ins, the reference
    // transfer plugin, two intake parsers, three notification destinations, three
    // post-processing steps, the WebDAV destination, two enrichers, six remote jobs and two
    // stream transforms. Two of the crawlers serve no provider at all
    // and name no domain in their manifest — the Nextcloud share and the open directory index
    // of RD-107-05 — because where somebody put their server is not a plugin author's to know.
    // A third, `peeplink-crawler`, names its two domains but serves no provider either: a link
    // protector is not a hoster and holds no account.
    //
    // Five providers account for fifteen of them: a cloud drive needs a resolver, a crawler
    // and an OAuth plugin, because a manifest carries exactly one `plugin_type` and only a
    // resolver may declare the `[provider]` row the other two hang off (RD-106-04). The shared
    // crates beside them — `common`, `guest`, `xfs-common`, `google-drive-common`,
    // `onedrive-common`, `dropbox-common`, `box-common`, `turbobit-common` — carry no manifest
    // and are not packaged. So does
    // `mega-login-probe` (RD-120-11), which is not a plugin at all but the measuring instrument
    // that priced a MEGA sign-in in guest fuel; it has no manifest for the same reason and is
    // built only by `scripts/measure-mega-login-fuel.sh`.
    //
    // An exact count so a plugin dropping out of the release is a test failure rather than a
    // quiet omission - and so adding one is a deliberate act. The fortieth is
    // `metadata-enricher` (RD-107-01); the forty-first and forty-second are
    // `directory-index-crawler` and `nextcloud-crawler` (RD-107-05). The forty-third is
    // `realdebrid-torrents` (RD-107-06), the first plugin of the eleventh world -- so
    // Real-Debrid now accounts for three of them: a resolver, an `oauth` sign-in and a
    // remote job, for the same reason the cloud drives account for three each. The
    // forty-fourth is `krakenfiles` (RD-103-08), an account-less hoster resolver; forty-fifth and
    // forty-sixth are `turbobit` and `hitfile` (RD-103-10, RD-103-11), two thin resolvers over the
    // shared `turbobit-common`, which carries no manifest either; forty-seventh and forty-eighth
    // are `mediafire` and `mediafire-crawler` (RD-103-06), a hoster whose folders are a crawler,
    // sharing `mediafire-common` and no account. The forty-ninth is `peeplink-crawler`
    // (RD-110-17): the one link-protection service of eight whose measurement carried, and the
    // third crawler that serves no provider and holds no account. The fiftieth is
    // `example-stream-transform` (RD-110-33), the reference plugin of the twelfth world: like
    // `example-transfer` and `example-oauth` it points at a host nobody can reach and exists to
    // be driven by the contract tests, because a usable one would be a plugin that fetches from
    // somebody's provider. The fifty-first and fifty-second are `mega` and `mega-crawler`
    // (RD-103-02): the first provider of that twelfth world and the crawler for its folders,
    // sharing `mega-common`, which carries no manifest either. MEGA has no resolver at all --
    // its addresses are claimed by the stream-transform plugin and the crawler, which is also
    // why `mega` carries the `[provider]` row since RD-120-20: there is no resolver to carry
    // it, and until it did, no MEGA account could be created at all. The fifty-third is
    // `mega-auth` (RD-120-20), the sign-in whose password the host computes over and the
    // plugin never sees. The fifty-fourth, fifty-fifth and fifty-sixth are `box`,
    // `box-crawler` and `box-oauth` (RD-120-05), the fourth cloud drive and therefore three
    // more at once, sharing `box-common`, which carries no manifest either. The fifty-seventh,
    // fifty-eighth and fifty-ninth are `torbox`, `torbox-auth` and `torbox-jobs` (RD-120-01):
    // the second provider of the eleventh world, and three at once for the same reason the
    // cloud drives are -- a manifest carries one `plugin_type`, only a resolver may declare
    // the `[provider]` row, and the remote job and the key check hang off its slug. They share
    // no crate: each of the three talks to a different part of TorBox's API, and the one thing
    // they agree on is a vault reference, which is a string in three manifests rather than a
    // dependency.
    // The sixtieth, sixty-first and sixty-second are `putio`, `putio-oauth` and
    // `putio-transfers` (RD-120-03): three again, and for the same two reasons at once -- only
    // the resolver may carry the `putio` provider row, and a remote job is a `plugin_type` of
    // its own. Unlike TorBox's three they do share a crate, `putio-common`, which carries no
    // manifest: the one address a Put.io file has is read at one end and written at the other,
    // and two copies of that would be two places for it to drift.
    // The sixty-third and sixty-fourth are `offcloud` and `offcloud-cloud` (RD-120-02): only
    // two rather than three, because Offcloud has no sign-in flow to run -- its key is typed,
    // so the credential slot lives in the resolver's `[provider]` row and no auth plugin
    // exists to carry one.
    // The sixty-fifth is `premiumize-transfers` (RD-120-23), the fifth provider of that world
    // and the fourth plugin on the Premiumize account -- one alone, because the resolver, the
    // sign-in and the crawler already existed. It does share a crate with the crawler,
    // `plugins/premiumize-common/`, which carries no manifest and is not counted: a finished
    // transfer's folder is exactly what the crawler walks, so `folder/list` is read in one
    // place rather than two.
    // The sixty-sixth, sixty-seventh and sixty-eighth are `pcloud`, `pcloud-crawler` and
    // `pcloud-oauth` (RD-120-06), the fifth cloud drive after Google Drive, OneDrive, Dropbox
    // and Box, and therefore the fifth set of three, sharing `pcloud-common`, which carries no
    // manifest either. The shared crate is not a convenience here: pCloud runs two separate
    // installations, and which one an address belongs to has to be read the same way at both
    // ends.
    // The sixty-ninth and seventieth are `pixeldrain` and `pixeldrain-crawler` (RD-120-07):
    // two rather than one, because Pixeldrain publishes two public link shapes and a resolver
    // answers with exactly one download -- `/u/` is a file and `/l/` is a collection, so the
    // list endpoint is a crawler. Two rather than three: the provider authenticates an API key
    // as an HTTP Basic password, which a plugin cannot assemble because it never holds its
    // credential, so the resolver's `[provider]` row carries `credentials = "none"` and there
    // is no sign-in plugin to add. They share no crate either -- the crawler reads one endpoint
    // the resolver never touches, and the one thing they agree on is which address shapes
    // belong to which, which is a test in each rather than a dependency.
    //
    // The seventy-first and seventy-second are `seedr` and `seedr-jobs` (RD-120-04), the sixth
    // provider of the eleventh world -- two rather than three, because Seedr has no sign-in
    // flow to run: its credential is the account's own e-mail address and password, typed into
    // the account, so the slot lives in the resolver's `[provider]` row and no auth plugin
    // exists to carry one. They share `plugins/seedr-common/`, which carries no manifest and is
    // not counted: the address of a Seedr file is written by one of them and read by the other,
    // and two copies of that would be two places for it to drift.
    //
    // Read this number, never add to a remembered one: every provider branch lands here and
    // the figure has been stale more than once. `ls plugins/*/manifest.toml | wc -l`.
    assert_eq!(directories.len(), 72, "expected 72 bundled plugins");
    directories
}

fn manifest_of(directory: &Path) -> rd_plugin_host::PluginManifest {
    let text = std::fs::read_to_string(directory.join("manifest.toml")).expect("manifest");
    toml::from_str(&text)
        .unwrap_or_else(|error| panic!("{} manifest is not valid v3: {error}", directory.display()))
}

/// Failure codes a plugin's `messages.rs` declares, in either shape:
/// `const X: (&str, &str) = ("code", "text")` or `const X: &str = "code"`.
///
/// Only the codes are collected. The English text is deliberately not compared: a translation
/// may carry `{param}` placeholders the backend fills in at runtime, so the catalogue entry
/// and the constant's fallback string are not expected to be identical.
fn declared_codes(directory: &Path) -> BTreeSet<String> {
    codes_declared_in(&directory.join("src/messages.rs"))
}

/// The codes one Rust source declares as constants, in the two shapes `declared_codes` names.
fn codes_declared_in(path: &Path) -> BTreeSet<String> {
    let Ok(source) = std::fs::read_to_string(path) else {
        return BTreeSet::new();
    };
    let mut messages = BTreeSet::new();
    // Constants wrap across lines and indent freely, so match on whitespace-normalised source.
    let flat = source.split_whitespace().collect::<Vec<_>>().join(" ");
    for chunk in flat.split("(&str, &str) = (").skip(1) {
        // The first two string literals after `= (` are the code and its English text. The
        // text itself may contain `);`, so it must not be used as a terminator.
        let Some(code) = chunk.split('"').nth(1) else {
            continue;
        };
        if code.contains('.') && !code.contains(' ') {
            messages.insert(code.to_owned());
        }
    }
    for chunk in flat.split("const ").skip(1) {
        let Some((declaration, rest)) = chunk.split_once(" = ") else {
            continue;
        };
        if !declaration.ends_with(": &str") {
            continue;
        }
        let Some(code) = rest.split('"').nth(1) else {
            continue;
        };
        if code.contains('.') && !code.contains(' ') {
            messages.insert(code.to_owned());
        }
    }
    messages
}

#[test]
fn every_bundled_plugin_ships_a_valid_v3_manifest_and_locales() {
    for directory in plugin_directories() {
        let manifest = manifest_of(&directory);
        let locales = directory.join("locales");
        assert!(
            locales.join("en.json").is_file(),
            "{} must ship locales/en.json",
            directory.display()
        );
        let mut files: Vec<(String, Vec<u8>)> = std::fs::read_dir(&locales)
            .expect("read locales")
            .filter_map(Result::ok)
            .filter_map(|entry| {
                let name = entry.file_name().to_string_lossy().into_owned();
                let language = name.strip_suffix(".json")?.to_owned();
                Some((language, std::fs::read(entry.path()).ok()?))
            })
            .collect();
        files.sort_by(|left, right| left.0.cmp(&right.0));
        rd_plugin_host::validate_locales(manifest.message_slug(), &files)
            .unwrap_or_else(|error| panic!("{} locales are invalid: {error}", directory.display()));
    }
}

/// Two plugins sharing an id is invisible in every other way: they install side by side under
/// one identity, the loader keeps only the highest version, and the plugin list reads like a
/// plugin that was updated. That is how the SponsorBlock enricher shipped unloaded for a whole
/// release (RD-098-03), so the collision gets a test of its own.
#[test]
fn no_two_bundled_plugins_share_an_id() {
    let mut owners: BTreeMap<String, PathBuf> = BTreeMap::new();
    for directory in plugin_directories() {
        let id = manifest_of(&directory).id.to_string();
        if let Some(other) = owners.insert(id.clone(), directory.clone()) {
            panic!(
                "{} and {} both declare plugin id {id}; the loader keeps only one of them",
                other.display(),
                directory.display()
            );
        }
    }
}

/// The core's own `server.codes` catalogue, which stays in the web bundle.
fn core_codes() -> BTreeMap<String, String> {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../web/src/locales/en/server.json");
    let text = std::fs::read_to_string(path).expect("core server.json");
    let value: serde_json::Value = serde_json::from_str(&text).expect("core server.json is JSON");
    value["codes"]
        .as_object()
        .expect("codes object")
        .iter()
        .map(|(code, text)| (code.clone(), text.as_str().unwrap_or_default().to_owned()))
        .collect()
}

#[test]
fn shipped_locales_cover_exactly_the_codes_the_resolver_emits() {
    let core = core_codes();
    for directory in plugin_directories() {
        let manifest = manifest_of(&directory);
        let slug = manifest.message_slug();
        let prefix = format!("{slug}.");
        let all_declared = declared_codes(&directory);
        if all_declared.is_empty() {
            continue;
        }
        // Codes outside the plugin's namespace are shared core codes (`link.*`, `plugin.*`);
        // they stay in the web bundle and must be translated there.
        for code in all_declared
            .iter()
            .filter(|code| !code.starts_with(&prefix))
        {
            assert!(
                core.contains_key(code),
                "{} emits `{code}`, which neither its own locales nor the core catalogue \
                 translate",
                directory.display()
            );
        }
        let declared: BTreeSet<String> = all_declared
            .into_iter()
            .filter(|code| code.starts_with(&prefix))
            .collect();
        let bytes = std::fs::read(directory.join("locales/en.json")).expect("en.json");
        let locale = rd_plugin_host::parse_locale(slug, "en", &bytes)
            .unwrap_or_else(|error| panic!("{} en.json invalid: {error}", directory.display()));

        for code in &declared {
            assert!(
                locale.codes.contains_key(code),
                "{} emits `{code}` but locales/en.json does not translate it",
                directory.display()
            );
        }
        for code in locale.codes.keys() {
            assert!(
                declared.contains(code),
                "{} translates `{code}`, which its messages.rs no longer emits",
                directory.display()
            );
        }
    }
}

#[test]
fn every_language_covers_the_same_codes_as_english() {
    for directory in plugin_directories() {
        let manifest = manifest_of(&directory);
        let english = rd_plugin_host::parse_locale(
            manifest.message_slug(),
            "en",
            &std::fs::read(directory.join("locales/en.json")).expect("en.json"),
        )
        .expect("en.json");

        for entry in std::fs::read_dir(directory.join("locales")).expect("locales") {
            let entry = entry.expect("entry");
            let name = entry.file_name().to_string_lossy().into_owned();
            let Some(language) = name.strip_suffix(".json").filter(|tag| *tag != "en") else {
                continue;
            };
            let locale = rd_plugin_host::parse_locale(
                manifest.message_slug(),
                language,
                &std::fs::read(entry.path()).expect("locale"),
            )
            .unwrap_or_else(|error| {
                panic!("{} {language}.json invalid: {error}", directory.display())
            });
            let missing: Vec<&String> = english
                .codes
                .keys()
                .filter(|code| !locale.codes.contains_key(*code))
                .collect();
            assert!(
                missing.is_empty(),
                "{} {language}.json is missing {missing:?}",
                directory.display()
            );
        }
    }
}

/// The account label parts every plugin says the same way are constructed once, in
/// `plugin-common`, with `plugin.account.*` codes. Those codes live in no plugin catalogue, so
/// only the core's `server.codes` can translate them -- and nothing else would notice a code
/// renamed in `label.rs` but not in `server.json`: the interface would print the English
/// fallback text in every language (RD-110-28).
#[test]
fn the_core_catalogue_translates_every_shared_account_label_code() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../plugins/common/src/label.rs");
    let declared = codes_declared_in(&path);
    assert!(
        declared.len() >= 8,
        "expected the shared label codes in {}, found {declared:?}",
        path.display()
    );
    let core = core_codes();
    for code in &declared {
        assert!(
            code.starts_with("plugin.account."),
            "{code} is a shared label code outside the core's `plugin.account.` namespace"
        );
        assert!(
            core.contains_key(code),
            "plugin-common emits `{code}`, which web/src/locales/en/server.json does not translate"
        );
    }
    // The host's own refusal of a part without a code is a core code too.
    assert!(core.contains_key("plugin.account_label_invalid"));
}
