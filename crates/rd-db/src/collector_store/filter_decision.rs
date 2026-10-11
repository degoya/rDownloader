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
            rd_core::LinkFilterAction::Route => Self {
                hidden_by: None,
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
                    }),
                )
            })
            .collect()
    }
}
