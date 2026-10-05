//! A search's type and ids, checked before anything is sent (RD-1100-03).
//!
//! Each id belongs to one search type -- a season means nothing to `t=movie` -- and one sent
//! with the wrong type is refused here rather than quietly dropped, so a person or an agent
//! learns that the search they asked for is not the one that would run.

use rd_subscription::TypedSearch;

use super::IndexerSearchRequest;
use crate::error::ApiError;

/// Longest IMDb id accepted, in digits; today's run to eight.
const MAX_IMDB_DIGITS: usize = 10;
/// Fewest digits an IMDb id has.
const MIN_IMDB_DIGITS: usize = 7;

/// The request's type and ids, checked; `tt` is taken off an IMDb id.
pub(crate) fn typed_input(request: &IndexerSearchRequest) -> Result<TypedSearch, ApiError> {
    let kind = request.search_type;
    let given: [(&str, bool); 6] = [
        ("season", request.season.is_some()),
        ("ep", request.episode.is_some()),
        ("tvdbid", request.tvdb_id.is_some()),
        ("tvmazeid", request.tvmaze_id.is_some()),
        ("imdbid", request.imdb_id.is_some()),
        ("tmdbid", request.tmdb_id.is_some()),
    ];
    if let Some((field, _)) = given
        .iter()
        .find(|(field, set)| *set && !kind.params().contains(field))
    {
        return Err(ApiError::unprocessable(
            "indexer.search_field_unsupported",
            "This search type does not take that field",
        )
        .with_param("field", *field)
        .with_param("type", kind.function()));
    }
    if request.episode.is_some() && request.season.is_none() {
        return Err(ApiError::unprocessable(
            "indexer.episode_without_season",
            "An episode needs its season",
        ));
    }
    for (field, value) in [
        ("tvdbid", request.tvdb_id),
        ("tvmazeid", request.tvmaze_id),
        ("tmdbid", request.tmdb_id),
    ] {
        if value == Some(0) {
            return Err(invalid_id(field));
        }
    }
    let imdb_id = request
        .imdb_id
        .as_deref()
        .map(imdb_digits)
        .transpose()?
        .flatten();
    Ok(TypedSearch {
        search_type: kind,
        season: request.season,
        episode: request.episode,
        tvdb_id: request.tvdb_id,
        tvmaze_id: request.tvmaze_id,
        imdb_id,
        tmdb_id: request.tmdb_id,
    })
}

/// The digits of `tt0903747` or `0903747`; `None` for a blank field.
fn imdb_digits(raw: &str) -> Result<Option<String>, ApiError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    let digits = trimmed
        .strip_prefix("tt")
        .or_else(|| trimmed.strip_prefix("TT"))
        .unwrap_or(trimmed);
    if (MIN_IMDB_DIGITS..=MAX_IMDB_DIGITS).contains(&digits.len())
        && digits.bytes().all(|byte| byte.is_ascii_digit())
    {
        Ok(Some(digits.to_owned()))
    } else {
        Err(invalid_id("imdbid"))
    }
}

fn invalid_id(field: &str) -> ApiError {
    ApiError::unprocessable("indexer.search_id_invalid", "That id is not a valid one")
        .with_param("field", field)
}

#[cfg(test)]
mod tests {
    use rd_subscription::IndexerSearchType;

    use super::{IndexerSearchRequest, typed_input};

    fn request(search_type: IndexerSearchType) -> IndexerSearchRequest {
        IndexerSearchRequest {
            search_type,
            ..IndexerSearchRequest::default()
        }
    }

    #[test]
    fn a_tv_search_takes_its_ids_and_the_imdb_id_loses_its_prefix() {
        let typed = typed_input(&IndexerSearchRequest {
            season: Some(1),
            episode: Some(2),
            tvdb_id: Some(81_189),
            tvmaze_id: Some(169),
            imdb_id: Some(" tt0903747 ".to_owned()),
            ..request(IndexerSearchType::Tv)
        })
        .expect("tv");
        assert_eq!(typed.imdb_id.as_deref(), Some("0903747"));
        assert_eq!(typed.season, Some(1));
        let movie = typed_input(&IndexerSearchRequest {
            imdb_id: Some("0133093".to_owned()),
            tmdb_id: Some(603),
            ..request(IndexerSearchType::Movie)
        })
        .expect("movie");
        assert_eq!(movie.tmdb_id, Some(603));
        let blank = typed_input(&IndexerSearchRequest {
            imdb_id: Some("  ".to_owned()),
            ..request(IndexerSearchType::Movie)
        })
        .expect("blank");
        assert_eq!(blank.imdb_id, None);
    }

    #[test]
    fn an_id_of_another_type_is_refused_by_name() {
        for (search_type, wrong) in [
            (
                IndexerSearchType::Movie,
                IndexerSearchRequest {
                    season: Some(1),
                    ..IndexerSearchRequest::default()
                },
            ),
            (
                IndexerSearchType::Tv,
                IndexerSearchRequest {
                    tmdb_id: Some(603),
                    ..IndexerSearchRequest::default()
                },
            ),
            (
                IndexerSearchType::Search,
                IndexerSearchRequest {
                    imdb_id: Some("tt0903747".to_owned()),
                    ..IndexerSearchRequest::default()
                },
            ),
        ] {
            let error = typed_input(&IndexerSearchRequest {
                search_type,
                ..wrong
            })
            .expect_err("refused");
            assert_eq!(error.code(), "indexer.search_field_unsupported");
        }
    }

    #[test]
    fn an_episode_needs_a_season_and_an_id_has_to_be_one() {
        let lone = typed_input(&IndexerSearchRequest {
            episode: Some(2),
            ..request(IndexerSearchType::Tv)
        })
        .expect_err("lone episode");
        assert_eq!(lone.code(), "indexer.episode_without_season");
        for imdb in ["tt12", "nm0000123", "tt09037470000000"] {
            let error = typed_input(&IndexerSearchRequest {
                imdb_id: Some(imdb.to_owned()),
                ..request(IndexerSearchType::Movie)
            })
            .expect_err(imdb);
            assert_eq!(error.code(), "indexer.search_id_invalid", "{imdb}");
        }
        let zero = typed_input(&IndexerSearchRequest {
            tvdb_id: Some(0),
            ..request(IndexerSearchType::Tv)
        })
        .expect_err("zero");
        assert_eq!(zero.code(), "indexer.search_id_invalid");
    }
}
