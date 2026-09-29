//! A scaffold metadata enricher. It compiles, packages and passes conformance as it is.
//!
//! This one reads what a release name already says — `Show.Name.S01E02.1080p.mkv` is season 1,
//! episode 2, 1080p — and adds it as fields of its own. It needs no network, so it asks for
//! none; an enricher that looks something up grants itself `[capabilities.net_http]` with the
//! exact domains and calls `http-request` from `guest`.
//!
//! Two things the host guarantees, which shape how this is written:
//!
//! - **Your fields are added, never substituted.** A name that collides with something the
//!   core already resolved is dropped and counted, so you cannot rewrite a file name or a
//!   size. Prefix every name with your slug and it never collides.
//! - **You are only asked once the person has switched enrichment on.** Nothing here runs for
//!   someone who did not ask for it, which is why it is safe for an enricher to reach outwards.
//!
//! The layout follows one practical concern: everything that can be tested without a
//! WebAssembly toolchain lives here, outside the component. `cargo test` in a fresh scaffold
//! runs it on the host target; `guest` exists only on `wasm32`.

#[cfg(target_arch = "wasm32")]
mod guest;

/// The namespace of every field this plugin adds: its own slug.
pub const PREFIX: &str = "{{PLUGIN_SLUG}}";

/// The fields a file name gives away, as `(name, value)` pairs.
///
/// Empty when it gives nothing away, which is an answer and not a failure: most links are not
/// episodes.
#[must_use]
pub fn fields(file_name: &str) -> Vec<(String, String)> {
    let mut found = Vec::new();
    // Release names separate words with dots, spaces, underscores or dashes.
    let words = file_name
        .split(['.', ' ', '_', '-'])
        .map(str::to_ascii_lowercase);
    for word in words {
        if let Some((season, episode)) = season_episode(&word) {
            found.push((format!("{PREFIX}.season"), season.to_string()));
            found.push((format!("{PREFIX}.episode"), episode.to_string()));
        } else if matches!(word.as_str(), "480p" | "720p" | "1080p" | "2160p") {
            found.push((format!("{PREFIX}.resolution"), word));
        }
    }
    found
}

/// `s01e02` as `(1, 2)`.
fn season_episode(word: &str) -> Option<(u32, u32)> {
    let (season, episode) = word.strip_prefix('s')?.split_once('e')?;
    let digits = |text: &str| -> Option<u32> {
        if text.is_empty() || text.len() > 3 || !text.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        text.parse().ok()
    };
    Some((digits(season)?, digits(episode)?))
}

#[cfg(test)]
mod tests {
    use super::{PREFIX, fields};

    fn field(name: &str, value: &str) -> (String, String) {
        (format!("{PREFIX}.{name}"), value.to_owned())
    }

    #[test]
    fn an_episode_name_gives_season_episode_and_resolution() {
        assert_eq!(
            fields("Show.Name.S01E02.1080p.WEB.mkv"),
            vec![
                field("season", "1"),
                field("episode", "2"),
                field("resolution", "1080p"),
            ]
        );
    }

    #[test]
    fn a_name_that_says_nothing_adds_nothing() {
        assert!(fields("holiday photos.zip").is_empty());
        assert!(fields("").is_empty());
        // Looks like an episode marker, is not one.
        assert!(fields("Spe.Special.mkv").is_empty());
    }

    #[test]
    fn every_field_is_in_this_plugins_namespace() {
        for (name, _) in fields("show_s10e100_720p.mkv") {
            assert!(name.starts_with(&format!("{PREFIX}.")), "{name}");
        }
    }
}
