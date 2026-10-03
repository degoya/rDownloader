use std::collections::BTreeMap;

use super::{BundledPackage, BundledText, ServiceCategory, group_services};
use crate::manifest::PluginManifest;

/// A package whose manifest is `header` plus `body`, as the bundle directory would hold it.
fn package(id: &str, plugin_type: &str, name: &str, version: &str, body: &str) -> BundledPackage {
    let manifest: PluginManifest = toml::from_str(&format!(
        r#"manifest_version = 3
plugin_type = "{plugin_type}"
api_version = "0.10.0"
id = "019d0000-0000-7000-8000-00000000{id}"
name = "{name}"
version = "{version}"
key_id = "dev"
public_key = "5C0fhOCoSaW9Ucdh1x3lUw05IX8YfNzJcgXkgnwjzeY="

[metadata]
description = "{name} fixture"
author = "rDownloader"

{body}"#
    ))
    .expect("fixture manifest");
    BundledPackage {
        path: format!("{name}.rdplug").into(),
        manifest,
        texts: BTreeMap::new(),
    }
}

fn provider(id: &str, name: &str, slug: &str, kind: &str, credentials: &str) -> BundledPackage {
    package(
        id,
        "resolver",
        name,
        "1.0.0",
        &format!(
            "[provider]\nslug = \"{slug}\"\nkind = \"{kind}\"\ncredentials = \"{credentials}\"\n"
        ),
    )
}

fn extension(id: &str, plugin_type: &str, name: &str, slug: &str, claims: &str) -> BundledPackage {
    package(
        id,
        plugin_type,
        name,
        "1.0.0",
        &format!("[extension]\nslug = \"{slug}\"\nclaims = [{claims}]\n"),
    )
}

#[test]
fn a_provider_and_the_extensions_that_claim_it_are_one_service() {
    let services = group_services(&[
        extension(
            "0a02",
            "crawler",
            "Vault folders",
            "vault_crawler",
            "\"vault\"",
        ),
        extension("0a03", "auth", "Vault sign-in", "vault_auth", "\"vault\""),
        provider("0a01", "Vault", "vault", "hoster", "username_password"),
    ]);
    assert_eq!(services.len(), 1);
    let service = &services[0];
    assert_eq!(service.key, "vault");
    assert_eq!(service.primary().manifest.name, "Vault");
    assert_eq!(service.packages.len(), 3);
    assert_eq!(service.category, ServiceCategory::Hoster);
    assert!(service.needs_account);
}

#[test]
fn categories_follow_the_provider_and_what_comes_with_it() {
    let services = group_services(&[
        provider("0b01", "Debrid", "debrid", "multihoster", "api_key"),
        extension(
            "0b02",
            "remote-job",
            "Debrid torrents",
            "debrid_jobs",
            "\"debrid\"",
        ),
        provider("0b03", "Seedbox", "seedbox", "hoster", "api_key"),
        extension(
            "0b04",
            "remote-job",
            "Seedbox jobs",
            "seedbox_jobs",
            "\"seedbox\"",
        ),
        provider("0b05", "Drive", "drive", "hoster", "oauth"),
        provider("0b06", "Freehost", "freehost", "hoster", "none"),
    ]);
    let category = |key: &str| {
        services
            .iter()
            .find(|service| service.key == key)
            .map(|service| service.category)
    };
    assert_eq!(category("debrid"), Some(ServiceCategory::Multihoster));
    assert_eq!(category("seedbox"), Some(ServiceCategory::RemoteJobs));
    assert_eq!(category("drive"), Some(ServiceCategory::Cloud));
    assert_eq!(category("freehost"), Some(ServiceCategory::Hoster));
    // Ordered by category: hosters first, then multihosters, remote jobs, cloud drives.
    let keys: Vec<&str> = services
        .iter()
        .map(|service| service.key.as_str())
        .collect();
    assert_eq!(keys, ["freehost", "debrid", "seedbox", "drive"]);
}

#[test]
fn only_what_needs_no_account_or_destination_is_account_free() {
    let services = group_services(&[
        provider("0c01", "Freehost", "freehost", "hoster", "none"),
        extension("0c02", "postprocess", "Checksums", "checksums", ""),
        extension("0c03", "intake", "Link lists", "link_lists", ""),
        extension("0c04", "notifier", "Chat", "chat_notifier", ""),
        extension("0c05", "storage", "Upload", "upload_storage", ""),
    ]);
    let free: Vec<&str> = services
        .iter()
        .filter(|service| !service.needs_account)
        .map(|service| service.key.as_str())
        .collect();
    assert_eq!(free, ["freehost", "link_lists", "checksums"]);
    let category = |key: &str| {
        services
            .iter()
            .find(|service| service.key == key)
            .map(|service| service.category)
    };
    assert_eq!(
        category("chat_notifier"),
        Some(ServiceCategory::Notifications)
    );
    assert_eq!(category("upload_storage"), Some(ServiceCategory::Cloud));
    assert_eq!(category("checksums"), Some(ServiceCategory::Postprocess));
}

#[test]
fn an_extension_claiming_a_provider_outside_the_bundle_is_its_own_service() {
    let services = group_services(&[extension(
        "0d01",
        "enricher",
        "Segments",
        "segments_enricher",
        "\"video.example\"",
    )]);
    assert_eq!(services.len(), 1);
    assert_eq!(services[0].key, "segments_enricher");
    assert_eq!(services[0].category, ServiceCategory::Metadata);
    assert!(!services[0].needs_account);
}

#[test]
fn two_versions_of_one_plugin_count_once_the_newest() {
    let older = package(
        "0e01",
        "postprocess",
        "Checksums",
        "1.0.0",
        "[extension]\nslug = \"checksums\"\n",
    );
    let newer = package(
        "0e01",
        "postprocess",
        "Checksums",
        "1.2.0",
        "[extension]\nslug = \"checksums\"\n",
    );
    let services = group_services(&[newer, older]);
    assert_eq!(services.len(), 1);
    assert_eq!(services[0].packages.len(), 1);
    assert_eq!(services[0].primary().manifest.version, "1.2.0");
}

#[test]
fn texts_fall_back_to_english_and_then_to_the_manifest() {
    let mut localised = extension("0f01", "postprocess", "Tidy names", "tidy", "");
    localised.texts.insert(
        "en".to_owned(),
        BundledText {
            name: Some("Tidy file names".to_owned()),
            description: None,
        },
    );
    localised.texts.insert(
        "de".to_owned(),
        BundledText {
            name: Some("Namen bereinigen".to_owned()),
            description: Some("Ersetzt Leerzeichen.".to_owned()),
        },
    );
    assert_eq!(localised.name("de"), "Namen bereinigen");
    assert_eq!(localised.name("fr"), "Tidy file names");
    assert_eq!(localised.description("de"), "Ersetzt Leerzeichen.");
    assert_eq!(localised.description("fr"), "Tidy names fixture");
}
