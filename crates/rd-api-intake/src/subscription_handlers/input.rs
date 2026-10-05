//! Validation of a subscription request into what the store accepts.

use super::*;

/// Validates the request and turns it into what the store accepts.
///
/// `api_key` is minted into the vault by the caller, because a failed insert has to remove
/// the reference again and only the caller knows whether the insert succeeded.
pub(crate) fn subscription_input(
    request: &SubscriptionRequest,
    secret_ref: Option<String>,
) -> Result<NewSubscription, ApiError> {
    let name = required_text(
        &request.name,
        TextLimit::Chars(MAX_NAME),
        "subscription.name_invalid",
        "A subscription needs a name",
    )?;
    let raw = required_text(
        &request.url,
        TextLimit::Bytes(MAX_URL),
        "subscription.url_invalid",
        "A subscription needs an address",
    )?;
    let url = if request.kind == SubscriptionKind::Script {
        script_url(&raw)?
    } else {
        let url = url::Url::parse(&raw).map_err(|_| {
            ApiError::bad_request("subscription.url_invalid", "Address is not a URL")
        })?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err(ApiError::bad_request(
                "subscription.url_scheme",
                "Only http and https addresses can be polled",
            ));
        }
        url
    };
    let schedule = schedule_input(request)?;
    let script_arguments = script_arguments_input(request)?;
    let indexer_search = indexer_search_input(request)?;
    let git_release = git_release_input(request, &url)?;
    // Refused rather than clamped: a person who typed 30 seconds should be told the limit,
    // not silently given something twenty times slower than they asked for. The floor is the
    // kind's (RD-110-21), because a board page is not an indexer -- `effective_interval`
    // clamps to the same number for anything that reaches the poller by another road.
    let minimum = request.kind.min_interval_seconds();
    if !(minimum..=MAX_POLL_INTERVAL_SECONDS).contains(&request.interval_seconds) {
        return Err(ApiError::unprocessable(
            "subscription.interval_invalid",
            "Poll interval is outside the permitted range",
        )
        .with_param("minimum", minimum.to_string())
        .with_param("maximum", MAX_POLL_INTERVAL_SECONDS.to_string()));
    }
    let filters = sanitize_filters(&request.filters)?;
    // Refused rather than defaulted: somebody who sent `21:9` asked for something, and
    // quietly drawing `2:1` instead would look like the setting did not take.
    let card_ratio =
        rd_core::SubscriptionCardRatio::parse(&request.card_ratio).ok_or_else(|| {
            ApiError::unprocessable(
                "subscription.card_ratio_unknown",
                "Card ratio is not one of 1:1, 3:2, 16:9, 4:3, 2:1",
            )
            .with_param(
                "value",
                request.card_ratio.chars().take(16).collect::<String>(),
            )
        })?;
    Ok(NewSubscription {
        source_categories: sanitize_source_categories(&request.source_categories)?,
        name,
        url,
        kind: request.kind,
        enabled: request.enabled,
        mode: request.mode,
        category_id: request.category_id,
        priority: request.priority,
        interval_seconds: request.interval_seconds,
        filters,
        backlog: request.backlog,
        category_map: sanitize_category_map(&request.category_map)?,
        every_release: request.every_release,
        view: request.view,
        autoplay: request.autoplay,
        card_ratio,
        schedule,
        script_arguments,
        indexer_search,
        git_release,
        secret_ref,
    })
}

/// Which release files a git-release subscription downloads (RD-190-13), checked.
///
/// The address is read the way the poller will read it, so a repository nobody can poll is a
/// form error and not a failure in the history.
pub(super) fn git_release_input(
    request: &SubscriptionRequest,
    url: &url::Url,
) -> Result<rd_core::GitReleaseOptions, ApiError> {
    let options = &request.git_release;
    if request.kind != SubscriptionKind::GitRelease {
        if options.is_empty() {
            return Ok(rd_core::GitReleaseOptions::default());
        }
        return Err(ApiError::unprocessable(
            "subscription.git_release_kind",
            "Only a git-release subscription takes release options",
        ));
    }
    let mut patterns: Vec<String> = Vec::new();
    for pattern in &options.asset_patterns {
        let pattern = pattern.trim();
        if pattern.is_empty() || patterns.iter().any(|kept| kept == pattern) {
            continue;
        }
        if pattern.chars().count() > rd_core::MAX_ASSET_PATTERN_CHARS {
            return Err(ApiError::unprocessable(
                "subscription.asset_pattern_too_long",
                "An asset name pattern is too long",
            )
            .with_param("maximum", rd_core::MAX_ASSET_PATTERN_CHARS.to_string()));
        }
        patterns.push(pattern.to_owned());
    }
    if patterns.len() > rd_core::MAX_ASSET_PATTERNS {
        return Err(ApiError::unprocessable(
            "subscription.asset_patterns_too_many",
            "Too many asset name patterns",
        )
        .with_param("maximum", rd_core::MAX_ASSET_PATTERNS.to_string()));
    }
    let host = url.host_str().unwrap_or_default();
    if options.forge.is_none() && rd_core::GitForge::of_host(host).is_none() {
        return Err(ApiError::unprocessable(
            "subscription.git_forge_unknown",
            "A self-hosted repository needs its forge named: GitHub or GitLab",
        )
        .with_param("host", host.chars().take(64).collect::<String>()));
    }
    rd_subscription::GitRepository::parse(url, options.forge).map_err(|_| {
        ApiError::unprocessable(
            "subscription.git_repository_invalid",
            "The address does not name a repository: owner and name are needed",
        )
    })?;
    Ok(rd_core::GitReleaseOptions {
        forge: options.forge,
        asset_patterns: patterns,
        platforms: distinct(&options.platforms),
        architectures: distinct(&options.architectures),
        prereleases: options.prereleases,
        source_archives: options.source_archives,
    })
}

/// The entries of `values` in their order, each once.
pub(super) fn distinct<T: Copy + PartialEq>(values: &[T]) -> Vec<T> {
    let mut kept = Vec::with_capacity(values.len());
    for value in values {
        if !kept.contains(value) {
            kept.push(*value);
        }
    }
    kept
}

/// The search an indexer subscription sends (RD-180-20), checked like an interactive search's.
pub(super) fn indexer_search_input(
    request: &SubscriptionRequest,
) -> Result<rd_core::IndexerSearch, ApiError> {
    if request.indexer_search.is_empty() {
        return Ok(rd_core::IndexerSearch::default());
    }
    if request.kind != SubscriptionKind::Indexer {
        return Err(ApiError::unprocessable(
            "subscription.search_kind",
            "Only an indexer subscription sends search parameters",
        ));
    }
    crate::indexer_handlers::search_input(&request.indexer_search)
}

/// Bounds the category map and drops entries with an empty source category.
///
/// A duplicate source category is kept as written rather than merged: the lookup takes the
/// first match, and silently discarding the second would hide a mistake the user can see.
/// Trims the requested categories, drops empties and duplicates, and caps the list.
///
/// Duplicates go here but deliberately not in `sanitize_category_map`: a repeated mapping is the
/// user's business, while a repeated `cat` value would just be sent twice to the indexer.
pub(crate) fn sanitize_source_categories(categories: &[String]) -> Result<Vec<String>, ApiError> {
    let mut cleaned: Vec<String> = Vec::new();
    for value in categories {
        let value = value.trim();
        if value.is_empty() || cleaned.iter().any(|kept| kept == value) {
            continue;
        }
        cleaned.push(value.to_owned());
    }
    if cleaned.len() > rd_core::MAX_CATEGORY_MAPPINGS {
        return Err(ApiError::unprocessable(
            "subscription.source_categories_too_many",
            "Too many indexer categories",
        )
        .with_param("maximum", rd_core::MAX_CATEGORY_MAPPINGS.to_string()));
    }
    Ok(cleaned)
}

pub(crate) fn sanitize_category_map(
    mappings: &[rd_core::CategoryMapping],
) -> Result<Vec<rd_core::CategoryMapping>, ApiError> {
    let cleaned: Vec<rd_core::CategoryMapping> = mappings
        .iter()
        .filter(|mapping| !mapping.source_category.trim().is_empty())
        .map(|mapping| rd_core::CategoryMapping {
            source_category: mapping.source_category.trim().to_owned(),
            category_id: mapping.category_id,
        })
        .collect();
    if cleaned.len() > rd_core::MAX_CATEGORY_MAPPINGS {
        return Err(ApiError::unprocessable(
            "subscription.category_map_too_many",
            "Too many category mappings",
        )
        .with_param("maximum", rd_core::MAX_CATEGORY_MAPPINGS.to_string()));
    }
    Ok(cleaned)
}

/// Trims and bounds the pattern lists, and refuses a range that can match nothing.
pub(super) fn sanitize_filters(
    filters: &SubscriptionFilters,
) -> Result<SubscriptionFilters, ApiError> {
    let clean = |values: &[String]| -> Result<Vec<String>, ApiError> {
        let cleaned: Vec<String> = values
            .iter()
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
            .collect();
        if cleaned.len() > MAX_FILTER_PATTERNS {
            return Err(ApiError::unprocessable(
                "subscription.filters_too_many",
                "Too many filter patterns",
            )
            .with_param("maximum", MAX_FILTER_PATTERNS.to_string()));
        }
        Ok(cleaned)
    };
    // An inverted range accepts nothing, and a subscription that silently accepts nothing is
    // exactly the failure this feature is supposed to make visible.
    if let (Some(minimum), Some(maximum)) =
        (filters.min_duration_seconds, filters.max_duration_seconds)
        && minimum > maximum
    {
        return Err(ApiError::unprocessable(
            "subscription.duration_range_invalid",
            "The shortest duration is longer than the longest",
        ));
    }
    Ok(SubscriptionFilters {
        title_contains: clean(&filters.title_contains)?,
        title_excludes: clean(&filters.title_excludes)?,
        languages: clean(&filters.languages)?,
        ..filters.clone()
    })
}
