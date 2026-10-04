//! The intake contract, exercised end to end against the two bundled parsers.
//!
//! Every test here is about a promise the host makes to the person pasting text, not to the
//! plugin: that a parser only sees input it claimed, that what it proposes is a suggestion
//! and not a queue entry, and that a parser reaches nothing its manifest did not ask for.
//!
//! The two parsers are here because they are unalike where it matters — one reads a
//! structured XML document, the other a flat key/value file written by another application —
//! so the contract is tested against two real formats rather than one invented one.

use rd_plugin_host::{PluginManifest, artifact::component, extension::IntakeParser};

fn metalink_manifest() -> PluginManifest {
    toml::from_str(include_str!(
        "../../../plugins/metalink-intake/manifest.toml"
    ))
    .expect("metalink manifest")
}

fn crawljob_manifest() -> PluginManifest {
    toml::from_str(include_str!(
        "../../../plugins/crawljob-intake/manifest.toml"
    ))
    .expect("crawljob manifest")
}

const META4: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<metalink xmlns="urn:ietf:params:xml:ns:metalink">
  <file name="example.iso">
    <size>14471447</size>
    <url priority="1">https://mirror.example/example.iso</url>
    <url priority="2">https://other.example/example.iso</url>
  </file>
</metalink>"#;

#[tokio::test]
async fn a_claimed_document_yields_the_files_it_lists() {
    let bytes = component("rd-plugin-metalink-intake");
    let parser = IntakeParser::new(metalink_manifest(), &bytes, None).expect("compile parser");

    let proposals = parser.parse(META4).await.expect("parse");
    assert_eq!(proposals.len(), 1, "{proposals:?}");
    assert_eq!(proposals[0].url, "https://mirror.example/example.iso");
    assert_eq!(proposals[0].file_name.as_deref(), Some("example.iso"));
    assert_eq!(proposals[0].size, Some(14_471_447));
    assert_eq!(proposals[0].package_hint.as_deref(), Some("metalink"));
}

#[tokio::test]
async fn an_input_the_parser_does_not_claim_is_never_shown_to_it() {
    let bytes = component("rd-plugin-metalink-intake");
    let parser = IntakeParser::new(metalink_manifest(), &bytes, None).expect("compile parser");

    // The host asks `claims` first. A parser for one format should not be handed every
    // paste in the application, and this is the mechanism that ensures it is not.
    let proposals = parser
        .parse("just some text with https://example.com/a in it")
        .await
        .expect("parse");
    assert!(proposals.is_empty(), "{proposals:?}");
}

#[tokio::test]
async fn a_malformed_entry_costs_only_itself() {
    let bytes = component("rd-plugin-metalink-intake");
    let parser = IntakeParser::new(metalink_manifest(), &bytes, None).expect("compile parser");

    // One entry this parser cannot use must not cost the person the rest of the document.
    let mixed = r#"<metalink xmlns="urn:ietf:params:xml:ns:metalink">
  <file name="torrent-only.iso"><url>magnet:?xt=urn:btih:abc</url></file>
  <file name="good.iso"><url>https://mirror.example/good.iso</url></file>
</metalink>"#;
    let proposals = parser.parse(mixed).await.expect("parse");
    assert_eq!(proposals.len(), 1, "{proposals:?}");
    assert_eq!(proposals[0].url, "https://mirror.example/good.iso");
}

#[tokio::test]
async fn a_crawljob_carries_its_package_name_as_a_hint() {
    let bytes = component("rd-plugin-crawljob-intake");
    let parser = IntakeParser::new(crawljob_manifest(), &bytes, None).expect("compile parser");

    let proposals = parser
        .parse("text=https://example.com/one.bin\npackageName=Holiday\nautoStart=TRUE\n")
        .await
        .expect("parse");
    assert_eq!(proposals.len(), 1, "{proposals:?}");
    assert_eq!(proposals[0].url, "https://example.com/one.bin");
    // A hint, not a package: what the host does with it is the host's decision.
    assert_eq!(proposals[0].package_hint.as_deref(), Some("Holiday"));
}

#[tokio::test]
async fn a_crawljob_cannot_direct_this_installation() {
    let bytes = component("rd-plugin-crawljob-intake");
    let parser = IntakeParser::new(crawljob_manifest(), &bytes, None).expect("compile parser");

    // A crawljob is written for another application and may say where to download to and
    // what to run afterwards. None of that survives intake here: what comes back is links.
    let proposals = parser
        .parse(
            "text=https://example.com/one.bin\ndownloadFolder=/etc\n\
             extractAfterDownload=TRUE\nautoStart=TRUE\n",
        )
        .await
        .expect("parse");
    assert_eq!(proposals.len(), 1, "{proposals:?}");
    assert_eq!(proposals[0].package_hint, None);
}

#[tokio::test]
async fn a_normalizer_that_has_nothing_to_tidy_says_so() {
    let bytes = component("rd-plugin-metalink-intake");
    let parser = IntakeParser::new(metalink_manifest(), &bytes, None).expect("compile parser");

    // No rewrite at all, rather than an identical string: the caller can then tell
    // "unchanged" from "rewritten to the same thing".
    assert_eq!(
        parser
            .normalize("https://example.com/a?id=7")
            .await
            .expect("normalize"),
        None
    );
}

#[tokio::test]
async fn a_parser_cannot_reach_anything_its_manifest_did_not_ask_for() {
    let bytes = component("rd-plugin-metalink-intake");
    // Both parsers declare no capabilities at all, so the host links only the base
    // interfaces. If one could still reach HTTP, the manifest would not be the thing that
    // decides what a plugin can do.
    for manifest in [metalink_manifest(), crawljob_manifest()] {
        assert!(manifest.capabilities.net_http.is_none());
        assert!(manifest.capabilities.net_stream.is_none());
        assert!(!manifest.capabilities.cookies);
        assert!(!manifest.capabilities.captcha);
    }
    assert!(IntakeParser::new(metalink_manifest(), &bytes, None).is_ok());
}

/// RD-150-03: the proposal is one link, and the set beside it is every mirror of the file.
#[tokio::test]
async fn the_metalink_parser_states_every_mirror_of_a_file() {
    let bytes = component("rd-plugin-metalink-intake");
    let parser = IntakeParser::new(metalink_manifest(), &bytes, None).expect("compile parser");
    assert!(parser.states_sources());

    let document = r#"<metalink xmlns="urn:ietf:params:xml:ns:metalink">
  <file name="example.iso">
    <size>14471447</size>
    <hash type="sha-256">0000000000000000000000000000000000000000000000000000000000000000</hash>
    <url priority="2" location="fr">https://other.example/example.iso</url>
    <url priority="1" location="de">https://mirror.example/example.iso</url>
    <url priority="3">ftp://ftp.example/example.iso</url>
  </file>
</metalink>"#;
    let proposals = parser.parse(document).await.expect("parse");
    assert_eq!(proposals[0].url, "https://mirror.example/example.iso");
    let sets = parser.source_sets(document).await.expect("sets");
    assert_eq!(sets.len(), 1, "{sets:?}");
    assert_eq!(sets[0].primary_url, proposals[0].url);
    let urls: Vec<_> = sets[0]
        .sources
        .iter()
        .map(|source| source.0.as_str())
        .collect();
    assert_eq!(
        urls,
        [
            "https://mirror.example/example.iso",
            "https://other.example/example.iso",
            "ftp://ftp.example/example.iso",
        ]
    );
    assert_eq!(sets[0].hashes.len(), 1);
}

#[tokio::test]
async fn a_parser_without_mirror_sets_keeps_its_plain_world() {
    // Crawljob exports `intake` only. It must still load, and must simply state nothing.
    let bytes = component("rd-plugin-crawljob-intake");
    let parser = IntakeParser::new(crawljob_manifest(), &bytes, None).expect("compile parser");
    assert!(!parser.states_sources());
    assert!(
        parser
            .source_sets("text=https://example.com/one.bin\n")
            .await
            .expect("sets")
            .is_empty()
    );
}

/// PLUG-16: a document the parser claims but cannot take one link from is refused, not
/// answered with an empty list that looks like success. The guest sends
/// `metalink_intake.unreadable`; this host carries code and message across, and `IntakeParsers`
/// logs it and lets the native scanner have the paste.
#[tokio::test]
async fn a_claimed_metalink_without_a_usable_file_is_refused() {
    let bytes = component("rd-plugin-metalink-intake");
    let parser = IntakeParser::new(metalink_manifest(), &bytes, None).expect("compile parser");

    let empty = r#"<metalink xmlns="urn:ietf:params:xml:ns:metalink">
  <file name="torrent-only.iso"><url>magnet:?xt=urn:btih:abc</url></file>
</metalink>"#;
    let error = parser
        .parse(empty)
        .await
        .expect_err("a claimed document with nothing in it is refused");
    assert!(
        error.to_string().contains("lists no usable file"),
        "{error:#}"
    );
    assert!(
        error
            .to_string()
            .starts_with("metalink_intake.unreadable: "),
        "the code reaches the log: {error:#}"
    );
}

/// PLUG-16, the crawljob half: claimed (it has `text=` and a crawljob-only key), no link.
#[tokio::test]
async fn a_claimed_crawljob_without_a_link_is_refused() {
    let bytes = component("rd-plugin-crawljob-intake");
    let parser = IntakeParser::new(crawljob_manifest(), &bytes, None).expect("compile parser");

    let error = parser
        .parse("text=see attachment\npackageName=Holiday\nautoStart=TRUE\n")
        .await
        .expect_err("a claimed crawljob with no link is refused");
    assert!(
        error.to_string().contains("carries no usable link"),
        "{error:#}"
    );
    assert!(
        error
            .to_string()
            .starts_with("crawljob_intake.unreadable: "),
        "the code reaches the log: {error:#}"
    );
    // Text it does not claim is still never shown to it, so an ordinary paste stays quiet.
    assert!(
        parser
            .parse("text=https://example.com/a")
            .await
            .expect("not claimed")
            .is_empty()
    );
}
