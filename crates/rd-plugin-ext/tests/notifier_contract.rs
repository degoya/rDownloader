//! The notification-destination contract, exercised against the three bundled plugins.
//!
//! What is checked here is not that a message arrives — that needs an ntfy server, a Discord
//! webhook and a Telegram bot, none of which belong in a test run. It is the promises the
//! host makes and the ones it demands: a destination reaches nothing outside its manifest, it
//! never holds a token, and it reports a failure rather than deciding to retry.

use rd_plugin_host::{PluginManifest, artifact::component, extension::NotifierPlugin};

fn manifest(source: &str) -> PluginManifest {
    toml::from_str(source).expect("bundled manifest")
}

const NTFY: &str = include_str!("../../../plugins/ntfy-notifier/manifest.toml");
const DISCORD: &str = include_str!("../../../plugins/discord-notifier/manifest.toml");
const TELEGRAM: &str = include_str!("../../../plugins/telegram-notifier/manifest.toml");

#[test]
fn every_destination_declares_exactly_the_service_it_talks_to() {
    // A notification destination is one service. A manifest listing several hosts would be a
    // plugin that could be pointed somewhere its name does not say. ntfy's `*` is the one
    // exception, and a narrow one: it stands for the server the destination names, and the
    // host cuts every delivery down to that single host (RD-130-15).
    for (source, expected) in [
        (NTFY, vec!["ntfy.sh", "*"]),
        (DISCORD, vec!["discord.com", "discordapp.com"]),
        (TELEGRAM, vec!["api.telegram.org"]),
    ] {
        let manifest = manifest(source);
        assert_eq!(manifest.capabilities.domains(), expected.as_slice());
        // Nothing else is asked for: no cookies, no captcha, no raw sockets.
        assert!(manifest.capabilities.net_stream.is_none());
        assert!(!manifest.capabilities.cookies);
        assert!(!manifest.capabilities.captcha);
        // And no vault reference: the one secret it may use is chosen per delivery by the
        // host, from the target being delivered to.
        assert!(
            manifest.capabilities.secrets.is_empty(),
            "{}",
            manifest.name
        );
    }
}

#[tokio::test]
async fn each_destination_compiles_against_the_notifier_world() {
    // The export side: a package that declares `plugin_type = "notifier"` but exports
    // something else fails here rather than the first time somebody presses "test".
    for (crate_name, source) in [
        ("rd-plugin-ntfy-notifier", NTFY),
        ("rd-plugin-discord-notifier", DISCORD),
        ("rd-plugin-telegram-notifier", TELEGRAM),
    ] {
        let bytes = component(crate_name);
        NotifierPlugin::new(manifest(source), &bytes, None)
            .unwrap_or_else(|error| panic!("{crate_name} does not satisfy the world: {error}"));
    }
}

#[tokio::test]
async fn a_destination_with_no_way_out_reports_a_failure_rather_than_hanging() {
    let bytes = component("rd-plugin-ntfy-notifier");
    // Built with no host: the plugin is linked, runs, and finds every request refused. What
    // matters is that this comes back as an error the hub can record — a plugin that could
    // block instead would hold up every other destination behind it.
    let plugin = NotifierPlugin::new(manifest(NTFY), &bytes, None).expect("compile");
    let error = plugin
        .deliver(rd_plugin_host::extension::Delivery {
            title: "Package finished",
            body: "example.iso",
            event: "package_completed",
            severity: "info",
            idempotency_key: "test:1",
            destination: "downloads",
            secret_ref: None,
        })
        .await
        .expect_err("no host is connected");
    assert!(
        error.to_string().contains("host"),
        "the failure should name the missing host: {error}"
    );
}

#[tokio::test]
async fn a_failure_keeps_the_category_the_plugin_gave_it() {
    // RD-120-62: the hub decides whether to retry from the plugin's own category, so the host
    // must not flatten a failure into a bare message on the way out. Telegram refuses a
    // destination with no stored token before it sends anything, as `auth_required`.
    let bytes = component("rd-plugin-telegram-notifier");
    let plugin = NotifierPlugin::new(manifest(TELEGRAM), &bytes, None).expect("compile");
    let error = plugin
        .deliver(rd_plugin_host::extension::Delivery {
            title: "Package finished",
            body: "example.iso",
            event: "package_completed",
            severity: "info",
            idempotency_key: "test:1",
            destination: "123456",
            secret_ref: None,
        })
        .await
        .expect_err("no token is stored");
    let failure = error
        .downcast_ref::<rd_core::Failure>()
        .unwrap_or_else(|| panic!("the plugin's failure arrives with its category: {error}"));
    assert_eq!(failure.category, rd_core::FailureKind::AuthRequired);
    assert!(!failure.category.is_retryable());
    assert_eq!(error.to_string(), failure.message);
}

/// RD-130-15: a destination the host would refuse at delivery is refused when the target is
/// saved, by the same decision and with the same code — the save handler asks
/// `check_destination`, and a delivery to the same address fails exactly as it said.
#[tokio::test]
async fn a_destination_is_judged_the_same_when_saved_and_when_delivered() {
    let bytes = component("rd-plugin-ntfy-notifier");
    let ntfy = NotifierPlugin::new(manifest(NTFY), &bytes, None).expect("compile");
    for accepted in [
        "downloads",
        "https://ntfy.example.org/alerts",
        "http://192.168.1.20/alerts",
        "http://ntfy:2586/alerts",
    ] {
        ntfy.check_destination(accepted)
            .unwrap_or_else(|failure| panic!("{accepted}: {failure}"));
    }

    let public_http = "http://ntfy.example.org/alerts";
    let saved = ntfy
        .check_destination(public_http)
        .expect_err("plain http to a public server");
    assert_eq!(
        saved.code.as_deref(),
        Some("plugin.destination_not_encrypted")
    );
    let error = ntfy
        .deliver(rd_plugin_host::extension::Delivery {
            title: "Package finished",
            body: "example.iso",
            event: "package_completed",
            severity: "info",
            idempotency_key: "test:saved-and-delivered",
            destination: public_http,
            secret_ref: None,
        })
        .await
        .expect_err("refused at delivery too");
    let delivered = error
        .downcast_ref::<rd_core::Failure>()
        .unwrap_or_else(|| panic!("the refusal carries its code: {error}"));
    assert_eq!(delivered.code, saved.code);

    // A destination whose manifest names its service has nothing to refuse here.
    let telegram = NotifierPlugin::new(
        manifest(TELEGRAM),
        &component("rd-plugin-telegram-notifier"),
        None,
    )
    .expect("compile");
    telegram
        .check_destination("http://anything.example/")
        .expect("Telegram's reach does not depend on the destination");
}
