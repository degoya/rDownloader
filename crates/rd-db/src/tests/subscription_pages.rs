//! A subscription's item pages read up to a watermark (RD-1250-13).

use super::subscriptions::{new_item, new_subscription};
use crate::Database;

#[tokio::test]
async fn a_page_until_a_watermark_leaves_out_items_found_after_it() {
    let directory = tempfile::tempdir().expect("tempdir");
    let database = Database::open(directory.path().join("subscription-until.sqlite"))
        .await
        .expect("database");
    let subscription = database
        .create_subscription(new_subscription("Indexer"))
        .await
        .expect("subscription");
    database
        .record_subscription_items(subscription.id, vec![new_item("a"), new_item("b")])
        .await
        .expect("first items");
    let pending = Some(rd_core::SubscriptionItemState::Pending);
    let seen = database
        .subscription_item_page(subscription.id, pending, 50, 0)
        .await
        .expect("first read");
    let watermark = seen.newest_discovered_at.expect("watermark");
    database
        .record_subscription_items(subscription.id, vec![new_item("later")])
        .await
        .expect("later item");

    let held = database
        .subscription_item_page_until(subscription.id, pending, 50, 0, Some(watermark))
        .await
        .expect("held read");
    let mut keys: Vec<_> = held
        .items
        .iter()
        .map(|item| item.item_key.as_str())
        .collect();
    keys.sort_unstable();
    assert_eq!(keys, ["a", "b"]);
    assert_eq!(held.total, 2);
    assert_eq!(held.newest_discovered_at, Some(watermark));

    let live = database
        .subscription_item_page_until(subscription.id, pending, 50, 0, None)
        .await
        .expect("live read");
    assert_eq!(live.total, 3);
    assert_eq!(live.items[0].item_key, "later");
}
