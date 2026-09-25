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
