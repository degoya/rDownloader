use super::{
    DEFAULT_POLL_INTERVAL_SECONDS, MAX_POLL_INTERVAL_SECONDS, MIN_POLL_INTERVAL_SECONDS,
    SITE_RULE_MIN_POLL_INTERVAL_SECONDS, Subscription, SubscriptionKind, SubscriptionMode,
};
use chrono::Utc;

fn subscription(interval: u32) -> Subscription {
    Subscription {
        id: crate::SubscriptionId::new(),
        name: "Channel".to_owned(),
        url: "https://example.test/c/x".parse().expect("url"),
        kind: SubscriptionKind::Media,
        enabled: true,
        mode: SubscriptionMode::Review,
        category_id: None,
        priority: crate::DownloadPriority::default(),
        interval_seconds: interval,
        filters: super::SubscriptionFilters::default(),
        backlog: super::BacklogPolicy::default(),
        category_map: Vec::new(),
        source_categories: Vec::new(),
        primed: false,
        last_run_at: None,
        next_run_at: None,
        consecutive_failures: 0,
        last_error: None,
        etag: None,
        last_modified: None,
        secret_ref: None,
        has_secret: false,
        every_release: false,
        view: super::SubscriptionView::List,
        autoplay: false,
        card_ratio: super::SubscriptionCardRatio::TwoOne,
        schedule: None,
        script_arguments: Vec::new(),
        indexer_search: crate::IndexerSearch::default(),
        git_release: crate::GitReleaseOptions::default(),
        created_at: Utc::now(),
        updated_at: Utc::now(),
    }
}

#[test]
fn an_interval_is_clamped_into_the_permitted_range() {
    // Polling somebody else's server every second gets the address blocked, and nothing
    // here is urgent enough to be worth that.
    assert_eq!(
        subscription(1).effective_interval(),
        MIN_POLL_INTERVAL_SECONDS
    );
    assert_eq!(
        subscription(u32::MAX).effective_interval(),
        MAX_POLL_INTERVAL_SECONDS
    );
    assert_eq!(
        subscription(DEFAULT_POLL_INTERVAL_SECONDS).effective_interval(),
        DEFAULT_POLL_INTERVAL_SECONDS
    );
}

#[test]
fn a_release_page_is_never_polled_faster_than_its_own_floor() {
    // RD-110-21. The floor is the kind's, so a row stored with the global minimum --
    // by an older build or by a person typing it -- is still polled politely.
    let mut watched = subscription(MIN_POLL_INTERVAL_SECONDS);
    watched.kind = SubscriptionKind::SiteRule;
    assert_eq!(
        watched.effective_interval(),
        SITE_RULE_MIN_POLL_INTERVAL_SECONDS
    );
    watched.interval_seconds = 1;
    assert_eq!(
        watched.effective_interval(),
        SITE_RULE_MIN_POLL_INTERVAL_SECONDS
    );
    // A longer interval than the floor is the person's business and stays.
    watched.interval_seconds = 6 * 60 * 60;
    assert_eq!(watched.effective_interval(), 6 * 60 * 60);
    // Every other kind keeps the global floor it always had.
    for kind in [
        SubscriptionKind::Media,
        SubscriptionKind::Gallery,
        SubscriptionKind::Feed,
        SubscriptionKind::Indexer,
    ] {
        assert_eq!(kind.min_interval_seconds(), MIN_POLL_INTERVAL_SECONDS);
    }
    const { assert!(SITE_RULE_MIN_POLL_INTERVAL_SECONDS > MIN_POLL_INTERVAL_SECONDS) };
}

#[test]
fn an_unmapped_category_falls_back_to_the_subscription_s_own() {
    // The behaviour that existed before mapping did, and what an indexer category
    // nobody has mapped yet has to keep doing.
    let default_category = crate::CategoryId::new();
    let mut subscription = subscription(600);
    subscription.category_id = Some(default_category);
    assert_eq!(subscription.category_for(None), Some(default_category));
    assert_eq!(
        subscription.category_for(Some("9999")),
        Some(default_category)
    );
}

#[test]
fn a_mapped_category_wins_over_the_default() {
    let default_category = crate::CategoryId::new();
    let tv = crate::CategoryId::new();
    let mut subscription = subscription(600);
    subscription.category_id = Some(default_category);
    subscription.category_map = vec![super::CategoryMapping {
        source_category: "5040".to_owned(),
        category_id: tv,
    }];
    assert_eq!(subscription.category_for(Some("5040")), Some(tv));
    // Only the mapped one: a near miss must not borrow another category's mapping.
    assert_eq!(
        subscription.category_for(Some("5030")),
        Some(default_category)
    );
}

#[test]
fn a_subscription_without_a_category_still_maps() {
    let tv = crate::CategoryId::new();
    let mut subscription = subscription(600);
    subscription.category_id = None;
    subscription.category_map = vec![super::CategoryMapping {
        source_category: "5040".to_owned(),
        category_id: tv,
    }];
    assert_eq!(subscription.category_for(Some("5040")), Some(tv));
    // Unmapped and no default: the routing rules decide, as they always did.
    assert_eq!(subscription.category_for(Some("2040")), None);
}

#[test]
fn a_script_has_no_backlog_and_names_its_script_under_its_own_scheme() {
    // RD-130-19. Every other kind keeps the backlog protection it always had.
    assert!(!SubscriptionKind::Script.has_backlog());
    for kind in [
        SubscriptionKind::Media,
        SubscriptionKind::Gallery,
        SubscriptionKind::Feed,
        SubscriptionKind::Indexer,
        SubscriptionKind::SiteRule,
    ] {
        assert!(kind.has_backlog(), "{kind:?}");
    }
    let mut script = subscription(DEFAULT_POLL_INTERVAL_SECONDS);
    script.kind = SubscriptionKind::Script;
    script.url = "script:daily-links.sh".parse().expect("url");
    assert_eq!(script.script_name(), Some("daily-links.sh"));
    // The same address on another kind names nothing, and so does an http address.
    script.kind = SubscriptionKind::Feed;
    assert_eq!(script.script_name(), None);
    script.kind = SubscriptionKind::Script;
    script.url = "https://example.test/links.sh".parse().expect("url");
    assert_eq!(script.script_name(), None);
    assert_eq!(
        serde_json::to_value(SubscriptionKind::Script).expect("serialise"),
        serde_json::json!("script")
    );
}

#[test]
fn review_is_the_default_mode() {
    // A subscription that starts queueing on its own is hard to undo.
    assert_eq!(SubscriptionMode::default(), SubscriptionMode::Review);
}

#[test]
fn the_view_defaults_to_the_list_and_round_trips_its_spelling() {
    use super::SubscriptionView;
    assert_eq!(SubscriptionView::default(), SubscriptionView::List);
    for view in [SubscriptionView::List, SubscriptionView::Cards] {
        assert_eq!(SubscriptionView::from_stored(view.as_str()), view);
        let json = serde_json::to_value(view).expect("serialise");
        assert_eq!(json, serde_json::Value::String(view.as_str().to_owned()));
    }
    assert_eq!(
        SubscriptionView::from_stored("carousel"),
        SubscriptionView::List
    );
}

#[test]
fn a_subscription_written_before_the_view_existed_reads_as_a_list_without_autoplay() {
    let mut json =
        serde_json::to_value(subscription(DEFAULT_POLL_INTERVAL_SECONDS)).expect("serialise");
    let object = json.as_object_mut().expect("object");
    object.remove("view");
    object.remove("autoplay");
    let read: Subscription = serde_json::from_value(json).expect("deserialise");
    assert_eq!(read.view, super::SubscriptionView::List);
    assert!(!read.autoplay);
}

#[test]
fn the_card_ratio_defaults_to_two_to_one_and_refuses_what_it_does_not_know() {
    use super::SubscriptionCardRatio;
    assert_eq!(
        SubscriptionCardRatio::default(),
        SubscriptionCardRatio::TwoOne
    );
    for ratio in SubscriptionCardRatio::ALL {
        assert_eq!(SubscriptionCardRatio::parse(ratio.as_str()), Some(ratio));
        assert_eq!(SubscriptionCardRatio::from_stored(ratio.as_str()), ratio);
        let json = serde_json::to_value(ratio).expect("serialise");
        assert_eq!(json, serde_json::Value::String(ratio.as_str().to_owned()));
    }
    // Portrait, asked for after the first live test of the card view.
    assert_eq!(
        SubscriptionCardRatio::parse("2:3"),
        Some(SubscriptionCardRatio::TwoThree)
    );
    for unknown in ["21:9", "2/1", "", " 2:1", "square"] {
        assert_eq!(SubscriptionCardRatio::parse(unknown), None, "{unknown:?}");
    }
    assert!(serde_json::from_value::<SubscriptionCardRatio>(serde_json::json!("21:9")).is_err());
    assert_eq!(
        SubscriptionCardRatio::from_stored("21:9"),
        SubscriptionCardRatio::TwoOne
    );
}

#[test]
fn a_subscription_written_before_the_card_ratio_existed_reads_as_two_to_one() {
    let mut json =
        serde_json::to_value(subscription(DEFAULT_POLL_INTERVAL_SECONDS)).expect("serialise");
    json.as_object_mut().expect("object").remove("card_ratio");
    let read: Subscription = serde_json::from_value(json).expect("deserialise");
    assert_eq!(read.card_ratio, super::SubscriptionCardRatio::TwoOne);
}
