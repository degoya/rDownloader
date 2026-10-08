//! The part list a multipart upload is completed with (TR-10).

use super::part_ids;

#[test]
fn every_part_with_an_id_completes_in_order() {
    let ids = part_ids(vec![Some("a".to_owned()), Some("b".to_owned())]).expect("all filled");
    let ids: Vec<&str> = ids.iter().map(|id| id.content_id.as_str()).collect();
    assert_eq!(ids, ["a", "b"]);
}

/// An empty slot used to become an empty ETag handed to the store; it is refused now and
/// names the first part without an id.
#[test]
fn a_part_without_an_id_is_refused_and_named() {
    assert_eq!(
        part_ids(vec![Some("a".to_owned()), None, None]).map(|ids| ids.len()),
        Err(1)
    );
}

/// RD-1190-20: a profile bound to one bucket uploads into that bucket only. Before, the binding
/// only filled an empty bucket, and a target could name any other one.
#[test]
fn a_bound_profile_reaches_no_other_bucket() {
    let s3 = rd_core::ObjectStorageProvider::S3;
    assert_eq!(
        super::split_destination(s3, "bound-bucket/in", Some("bound-bucket")),
        Some(("bound-bucket", "in"))
    );
    assert_eq!(
        super::split_destination(s3, "other-bucket/in", Some("bound-bucket")),
        None
    );
}
