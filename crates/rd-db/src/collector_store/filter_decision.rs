//! What the LinkFilter decides about each link of a batch before it is grouped (RD-1240-09).

use super::NewCollectorBatch;

/// What the first matching LinkFilter rule said about one link (RD-1240-09).
#[derive(Default)]
pub(super) struct FilterDecision {
    /// The hiding rule; the link is kept, behind the list's "Show hidden" switch.
    pub(super) hidden_by: Option<rd_core::LinkFilterRuleId>,
    /// The package a `route` rule puts the link in; it outranks the source's package hint.
    pub(super) package: Option<String>,
    /// The category a `route` rule gives the link's package.
    pub(super) category: Option<rd_core::CategoryId>,
    /// The deleting rule (RD-1310-01); the link is not written at all.
    pub(super) deleted_by: Option<rd_core::LinkFilterRuleId>,
}

impl FilterDecision {
    fn of(rule: Option<&rd_core::LinkFilterRule>) -> Self {
        let Some(rule) = rule else {
            return Self::default();
        };
        match rule.action {
            rd_core::LinkFilterAction::Hide => Self {
                hidden_by: Some(rule.id),
                ..Self::default()
            },
            rd_core::LinkFilterAction::Accept => Self::default(),
            rd_core::LinkFilterAction::Delete => Self {
                deleted_by: Some(rule.id),
                ..Self::default()
            },
            rd_core::LinkFilterAction::Route => Self {
                hidden_by: None,
                deleted_by: None,
                package: rule
                    .package_name
                    .as_deref()
                    .map(str::trim)
                    .filter(|name| !name.is_empty())
                    .map(str::to_owned),
                category: rule.category_id,
            },
        }
    }

    /// One decision per link of `intake`, parallel to `urls`; `file_names` is what the intake
    /// settled on for each, a name the source gave or the address's last segment.
    pub(super) fn batch(
        intake: &NewCollectorBatch,
        filters: &rd_collector::LinkFilters<'_>,
        file_names: &[Option<String>],
    ) -> Vec<Self> {
        intake
            .urls
            .iter()
            .enumerate()
            .map(|(index, url)| {
                if filters.is_empty() {
                    return Self::default();
                }
                Self::of(
                    filters.decide(&rd_collector::LinkFilterContext {
                        source: intake.source,
                        url,
                        file_name: file_names.get(index).and_then(Option::as_deref),
                        size: intake
                            .sizes
                            .get(index)
                            .copied()
                            .flatten()
                            .map(|size| size.get()),
                        // Nothing is checked at intake yet (RD-1310-01).
                        unresolvable_without_resolver: false,
                    }),
                )
            })
            .collect()
    }

    /// The vault references of the links a deleting rule kept out of `intake` (RD-1310-01):
    /// nothing points at them once the link is not written. Each such link is logged at debug
    /// level.
    pub(super) fn deleted_refs(
        decisions: &[Self],
        intake: &NewCollectorBatch,
        fragment_refs: &[Option<String>],
    ) -> Vec<String> {
        let mut orphaned = Vec::new();
        for (index, decision) in decisions.iter().enumerate() {
            let Some(rule) = decision.deleted_by else {
                continue;
            };
            tracing::debug!(
                rule_id = %rule,
                host = intake.urls.get(index).and_then(|url| url.host_str()).unwrap_or_default(),
                "a LinkFilter rule deleted a link at intake"
            );
            orphaned.extend(fragment_refs.get(index).cloned().flatten());
            orphaned.extend(intake.body_refs.get(index).cloned().flatten());
        }
        orphaned
    }
}
