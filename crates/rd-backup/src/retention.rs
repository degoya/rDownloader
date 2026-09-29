//! Which archives a destination keeps (RD-160-02).
//!
//! A destination keeps the newest `keep_last` archives and every archive younger than
//! `keep_days`; with both set an archive stays when either rule wants it, with neither set
//! every archive stays. The newest archive always stays, whatever the rules say — retention
//! never leaves a destination without its last backup.
//!
//! What may go at all is decided before any rule is read: only archives this installation
//! recorded as its own at that destination (the `backup_archives` ledger), and only under a
//! name that carries this installation's id ([`is_own_archive`]). A file somebody else put
//! there, an archive of another installation writing to the same folder, a file renamed by
//! hand — none of them is a candidate, so none of them can be deleted.

use chrono::{DateTime, Duration, Utc};

/// How long and how many archives a destination keeps. `None` is no limit of that kind.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RetentionPolicy {
    pub keep_last: Option<u32>,
    pub keep_days: Option<u32>,
}

impl RetentionPolicy {
    /// Whether the policy removes anything ever.
    #[must_use]
    pub const fn is_unlimited(&self) -> bool {
        self.keep_last.is_none() && self.keep_days.is_none()
    }
}

/// One archive the ledger holds for a destination.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecordedArchive {
    pub id: String,
    pub name: String,
    /// When the run that wrote it started; the archive's point in time.
    pub created_at: DateTime<Utc>,
}

/// What a retention pass does, as ids of [`RecordedArchive`]s, newest first.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RetentionPlan {
    pub keep: Vec<String>,
    pub remove: Vec<String>,
}

/// Whether `name` is an archive this installation writes: the name [`crate::archive_name`]
/// gives it, with `instance` in it.
#[must_use]
pub fn is_own_archive(name: &str, instance: &str) -> bool {
    !instance.is_empty()
        && name
            .strip_prefix(crate::create::ARCHIVE_PREFIX)
            .and_then(|rest| rest.strip_prefix(instance))
            .and_then(|rest| rest.strip_prefix('-'))
            .and_then(|rest| rest.strip_suffix(crate::ARCHIVE_EXTENSION))
            .is_some_and(|stamp| stamp.len() > 1 && stamp.ends_with('.'))
}

/// Plans a retention pass over the ledger's archives of one destination at `now`.
#[must_use]
pub fn plan(
    archives: &[RecordedArchive],
    policy: RetentionPolicy,
    instance: &str,
    now: DateTime<Utc>,
) -> RetentionPlan {
    let mut ordered: Vec<&RecordedArchive> = archives.iter().collect();
    ordered.sort_by(|left, right| {
        right
            .created_at
            .cmp(&left.created_at)
            .then_with(|| right.name.cmp(&left.name))
    });
    let cutoff = policy
        .keep_days
        .map(|days| now - Duration::days(i64::from(days)));
    let mut plan = RetentionPlan::default();
    let mut own_seen = 0_u32;
    for archive in ordered {
        if !is_own_archive(&archive.name, instance) {
            plan.keep.push(archive.id.clone());
            continue;
        }
        own_seen += 1;
        let newest = own_seen == 1;
        let by_count = policy.keep_last.is_some_and(|last| own_seen <= last);
        let by_age = cutoff.is_some_and(|cutoff| archive.created_at >= cutoff);
        if newest || policy.is_unlimited() || by_count || by_age {
            plan.keep.push(archive.id.clone());
        } else {
            plan.remove.push(archive.id.clone());
        }
    }
    plan
}

#[cfg(test)]
mod tests {
    use chrono::{Duration, TimeZone, Utc};
    use rand::RngExt;

    use super::{RecordedArchive, RetentionPolicy, is_own_archive, plan};
    use crate::archive_name;

    const INSTANCE: &str = "0a1b2c3d";

    fn archive(id: usize, hours_ago: i64, own: bool) -> RecordedArchive {
        let now = Utc
            .with_ymd_and_hms(2026, 9, 28, 12, 0, 0)
            .single()
            .expect("now");
        let created_at = now - Duration::hours(hours_ago);
        RecordedArchive {
            id: format!("a{id}"),
            name: if own {
                archive_name(INSTANCE, created_at)
            } else {
                archive_name("ffffffff", created_at)
            },
            created_at,
        }
    }

    fn now() -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, 28, 12, 0, 0)
            .single()
            .expect("now")
    }

    #[test]
    fn a_name_is_own_only_with_this_instance_in_it() {
        let name = archive_name(INSTANCE, now());
        assert!(is_own_archive(&name, INSTANCE));
        assert!(!is_own_archive(&name, "ffffffff"));
        assert!(!is_own_archive(&name, ""));
        assert!(!is_own_archive(
            "rdownloader-backup-0a1b2c3d-.rdbackup",
            INSTANCE
        ));
        assert!(!is_own_archive("holiday-photos.rdbackup", INSTANCE));
        assert!(!is_own_archive(&format!("{name}.partial"), INSTANCE));
    }

    #[test]
    fn the_newest_n_and_the_young_ones_stay() {
        let archives: Vec<_> = (0..6).map(|id| archive(id, id as i64 * 24, true)).collect();
        let by_count = plan(
            &archives,
            RetentionPolicy {
                keep_last: Some(2),
                keep_days: None,
            },
            INSTANCE,
            now(),
        );
        assert_eq!(by_count.keep, ["a0", "a1"]);
        assert_eq!(by_count.remove, ["a2", "a3", "a4", "a5"]);
        let by_age = plan(
            &archives,
            RetentionPolicy {
                keep_last: None,
                keep_days: Some(3),
            },
            INSTANCE,
            now(),
        );
        assert_eq!(by_age.keep, ["a0", "a1", "a2", "a3"]);
        // Either rule keeps an archive.
        let both = plan(
            &archives,
            RetentionPolicy {
                keep_last: Some(5),
                keep_days: Some(1),
            },
            INSTANCE,
            now(),
        );
        assert_eq!(both.remove, ["a5"]);
        assert!(
            plan(&archives, RetentionPolicy::default(), INSTANCE, now())
                .remove
                .is_empty()
        );
    }

    #[test]
    fn the_newest_archive_stays_even_when_every_rule_would_remove_it() {
        let archives = vec![archive(0, 24 * 90, true), archive(1, 24 * 100, true)];
        let old = plan(
            &archives,
            RetentionPolicy {
                keep_last: None,
                keep_days: Some(7),
            },
            INSTANCE,
            now(),
        );
        assert_eq!(old.keep, ["a0"]);
        assert_eq!(old.remove, ["a1"]);
    }

    /// Randomised ledgers against the invariants: nothing foreign is ever removed, the newest
    /// own archive always stays, every id lands in exactly one list, and nothing the rules
    /// want kept is removed.
    #[test]
    fn retention_never_removes_what_it_must_keep() {
        let mut rng = rand::rng();
        for _ in 0..500 {
            let count = rng.random_range(0..25_usize);
            let archives: Vec<_> = (0..count)
                .map(|id| archive(id, rng.random_range(0..24 * 60), rng.random_range(0..4) > 0))
                .collect();
            let policy = RetentionPolicy {
                keep_last: rng.random_bool(0.6).then(|| rng.random_range(1..8)),
                keep_days: rng.random_bool(0.6).then(|| rng.random_range(1..30)),
            };
            let result = plan(&archives, policy, INSTANCE, now());
            assert_eq!(result.keep.len() + result.remove.len(), archives.len());
            let removed = |archive: &RecordedArchive| result.remove.contains(&archive.id);
            for candidate in &archives {
                if !is_own_archive(&candidate.name, INSTANCE) {
                    assert!(!removed(candidate), "a foreign archive was removed");
                }
                if let Some(days) = policy.keep_days
                    && candidate.created_at >= now() - Duration::days(i64::from(days))
                {
                    assert!(!removed(candidate), "a young archive was removed");
                }
            }
            let mut own: Vec<_> = archives
                .iter()
                .filter(|candidate| is_own_archive(&candidate.name, INSTANCE))
                .collect();
            own.sort_by(|left, right| {
                right
                    .created_at
                    .cmp(&left.created_at)
                    .then_with(|| right.name.cmp(&left.name))
            });
            if let Some(newest) = own.first() {
                assert!(!removed(newest), "the newest archive was removed");
            }
            if let Some(last) = policy.keep_last {
                for kept in own.iter().take(last as usize) {
                    assert!(!removed(kept), "one of the newest {last} was removed");
                }
            }
            if policy.is_unlimited() {
                assert!(result.remove.is_empty());
            }
        }
    }
}
