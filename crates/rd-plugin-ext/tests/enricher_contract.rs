//! The metadata-enricher contract, exercised against the bundled SponsorBlock plugin.
//!
//! The promise worth checking is the one a plugin cannot be trusted to keep on its own: that
//! what it returns is *added* to a link and never replaces what the application resolved
//! itself. That is enforced in the adapter, so it is tested there — against a plugin that
//! deliberately tries.

use std::sync::Arc;

use rd_plugin_host::{PluginManifest, artifact::component, extension::MetadataEnricher};

const SPONSORBLOCK: &str = include_str!("../../../plugins/sponsorblock-enricher/manifest.toml");

fn manifest() -> PluginManifest {
    toml::from_str(SPONSORBLOCK).expect("bundled manifest")
}

#[tokio::test]
async fn the_enricher_compiles_against_its_world() {
    let bytes = component("rd-plugin-sponsorblock-enricher");
    MetadataEnricher::new(manifest(), &bytes, None).expect("satisfies the world");
}

#[test]
fn the_enricher_reaches_one_service_and_claims_one_site() {
    // Both halves matter. One domain, so it cannot ask anywhere else; and a claim list, so a
    // link it could not possibly know about is never sent to that service at all.
    let manifest = manifest();
    assert_eq!(manifest.capabilities.domains(), ["sponsor.ajay.app"]);
    let claims = manifest
        .extension
        .as_ref()
        .map(|extension| extension.claims.clone())
        .unwrap_or_default();
    assert_eq!(
        claims,
        vec!["youtube.com".to_owned(), "youtu.be".to_owned()]
    );
    assert!(manifest.capabilities.secrets.is_empty());
    assert!(!manifest.capabilities.cookies);
}

#[tokio::test]
async fn an_enricher_with_no_way_out_costs_the_link_nothing() {
    let bytes = component("rd-plugin-sponsorblock-enricher");
    // Built with no host, so every request is refused. The link still resolved; the enricher
    // failing must not turn a perfectly good candidate into one somebody has to think about.
    let enricher = Arc::new(MetadataEnricher::new(manifest(), &bytes, None).expect("compile"));
    let result = enricher
        .enrich("https://www.youtube.com/watch?v=dQw4w9WgXcQ", None, None)
        .await;
    assert!(result.is_err(), "the request should be refused");
}

// -- the metadata enricher (RD-107-01) ------------------------------------------------------
//
// The second plugin of this type, and the first that is asked about *every* link: an indexer
// hit's URL points at the indexer, so the domain says nothing about what the link contains and
// `claims` cannot narrow anything. What is checked here is therefore the other half of the
// promise SponsorBlock keeps with a claim list — that the commonest answer, "this is not a
// film", is reached without telling any service about the link at all — and that a source
// which does not answer, or answers with nonsense, costs the candidate nothing.

use async_trait::async_trait;
use rd_core::{AccountId, Failure as CoreFailure, FailureKind};
use rd_plugin_api::{ClientIdentity, HostHttpRequest, HostHttpResponse};
use std::sync::Mutex;

const METADATA: &str = include_str!("../../../plugins/metadata-enricher/manifest.toml");

fn metadata_manifest() -> PluginManifest {
    toml::from_str(METADATA).expect("bundled manifest")
}

/// A film release, and the answer the source gives for it.
const FILM: &str = "Blade.Runner.2049.2017.2160p.UHD.BluRay.x265-GRP.mkv";
const CATALOGUE: &str = r#"{"metas":[
    {"id":"tt0083658","type":"movie","name":"Blade Runner","releaseInfo":"1982",
     "imdbRating":"8.1","genres":["Action","Drama","Sci-Fi"]},
    {"id":"tt1856101","type":"movie","name":"Blade Runner 2049","releaseInfo":"2017",
     "imdbRating":"8.0","genres":["Action","Drama","Mystery"]}
]}"#;
const DETAIL: &str = r#"{"meta":{"id":"tt1856101","type":"movie","name":"Blade Runner 2049",
    "releaseInfo":"2017","imdbRating":"8.0","runtime":"164 min","genres":["Action"]}}"#;
const SERIES_CATALOGUE: &str = r#"{"metas":[{"id":"tt0903747","type":"series",
    "name":"Breaking Bad","releaseInfo":"2008-2013","imdbRating":"9.5",
    "genres":["Crime","Drama"],"runtime":"49 min"}]}"#;

/// An episode of one series, and the answer Cinemeta's fuzzy search really gives for it: a
/// different series whose name merely sounds alike (RD-108-14).
const APOLLO: &str = "Apollo.Has.Fallen.S02E02.DL.GERMAN.1080p.WEB.h264-GRP.mkv";
const WRONG_SERIES: &str = r#"{"metas":[{"id":"tt21371866","type":"series",
    "name":"Paris Has Fallen","releaseInfo":"2024","imdbRating":"6.5",
    "genres":["Action","Drama"],"runtime":"48 min"}]}"#;

/// An episode numbered straight through, with no season anywhere in the name, and the film the
/// film catalogue used to answer with when it was read as one.
const DAIMA: &str = "Dragon.Ball.DAIMA.E15.Das.dritte.Auge.German.DL.1080p.WEB.h264-GRP.mkv";
const WRONG_FILM: &str = r#"{"metas":[{"id":"tt0142242","type":"movie",
    "name":"Dragon Ball Z: Broly - Second Coming","releaseInfo":"1994","imdbRating":"6.5",
    "genres":["Animation"],"runtime":"48 min"}]}"#;

/// What the mock source does with a request.
enum Answer {
    /// The catalogue body, then the detail body.
    Bodies(&'static str, &'static str),
    /// Nothing comes back at all — the source is down, or too slow to wait for.
    Silence,
}

/// The mock source. It answers at the host boundary, so no socket is opened and
/// `v3-cinemeta.strem.io` is never contacted.
///
/// Its second job is to count: a test that asserts "no request was made" needs somewhere the
/// absence is visible, and this is it.
struct MockSource {
    answer: Answer,
    requests: Mutex<Vec<String>>,
}

impl MockSource {
    fn new(answer: Answer) -> Arc<Self> {
        Arc::new(Self {
            answer,
            requests: Mutex::new(Vec::new()),
        })
    }

    fn requests(&self) -> Vec<String> {
        self.requests.lock().expect("requests").clone()
    }
}

#[async_trait]
impl rd_plugin_api::ResolverHost for MockSource {
    async fn http_request(
        &self,
        _client: &ClientIdentity,
        request: HostHttpRequest,
    ) -> Result<HostHttpResponse, CoreFailure> {
        self.requests
            .lock()
            .expect("requests")
            .push(request.url.to_string());
        let (catalogue, detail) = match self.answer {
            Answer::Bodies(catalogue, detail) => (catalogue, detail),
            Answer::Silence => {
                return Err(CoreFailure::coded(
                    FailureKind::Transient {
                        retry_after_seconds: None,
                    },
                    "test.no_answer",
                    "the source did not answer in time",
                ));
            }
        };
        let body = if request.url.path().starts_with("/meta/") {
            detail
        } else {
            catalogue
        };
        Ok(HostHttpResponse {
            status: 200,
            final_url: request.url.clone(),
            headers: Vec::new(),
            body: body.as_bytes().to_vec(),
        })
    }

    async fn secret_available(&self, _account_id: AccountId, _reference: &str) -> bool {
        false
    }
}

/// Builds the enricher against a mock source, failing when the component is not built here.
fn metadata_enricher(source: Arc<MockSource>) -> MetadataEnricher {
    let bytes = rd_plugin_host::artifact::component("rd-plugin-metadata-enricher");
    MetadataEnricher::new(metadata_manifest(), &bytes, Some(source)).expect("compile")
}

fn field<'a>(fields: &'a [(String, String)], name: &str) -> Option<&'a str> {
    fields
        .iter()
        .find(|(field, _)| field == name)
        .map(|(_, value)| value.as_str())
}

#[tokio::test]
async fn the_metadata_enricher_compiles_against_its_world() {
    let bytes = rd_plugin_host::artifact::component("rd-plugin-metadata-enricher");
    MetadataEnricher::new(metadata_manifest(), &bytes, None).expect("satisfies the world");
}

#[test]
fn the_metadata_enricher_reaches_one_keyless_source_and_claims_nothing() {
    let manifest = metadata_manifest();
    // One address. An enricher is instantiated without an account, so the host expands no
    // `{{secret:...}}` here — a source wanting an API key could not be reached at all, and
    // the empty secret list is what says this one does not want one.
    assert_eq!(manifest.capabilities.domains(), ["v3-cinemeta.strem.io"]);
    assert!(manifest.capabilities.secrets.is_empty());
    assert!(!manifest.capabilities.cookies);
    assert!(!manifest.capabilities.captcha);
    // And an empty claim list, deliberately: the URL of an indexer hit points at the indexer,
    // so no domain pattern could decide this. The plugin decides from the file name instead.
    let claims = manifest
        .extension
        .as_ref()
        .map(|extension| extension.claims.clone())
        .unwrap_or_default();
    assert!(claims.is_empty(), "{claims:?}");
}

#[test]
fn the_two_bundled_enrichers_do_not_share_an_identity() {
    // The lesson of RD-098-03: two plugins with one id means the loader keeps the higher
    // version and says nothing about the other, which simply never runs.
    assert_ne!(manifest().id, metadata_manifest().id);
    assert_ne!(
        manifest().extension.expect("extension").slug,
        metadata_manifest().extension.expect("extension").slug
    );
}

#[tokio::test]
async fn a_name_that_is_not_a_film_reaches_the_source_not_at_all() {
    let source = MockSource::new(Answer::Bodies(CATALOGUE, DETAIL));
    let enricher = metadata_enricher(Arc::clone(&source));
    // The commonest case by a wide margin, and the one that has to be free. Every one of
    // these is a link this plugin is asked about, and none of them may leave the machine.
    for name in [
        "setup.exe",
        "holiday-photos.zip",
        "invoice.pdf",
        "readme.nfo",
        "Some.Film.2019.1080p.BluRay.x264-GRP.srt",
        "holiday.mkv",
        "document",
    ] {
        let fields = enricher
            .enrich("https://indexer.example/dl/abcdef", Some(name), None)
            .await
            .expect("an enricher never fails a link");
        assert!(fields.is_empty(), "{name} produced {fields:?}");
    }
    assert!(
        source.requests().is_empty(),
        "nothing may be asked: {:?}",
        source.requests()
    );
}

#[tokio::test]
async fn a_recognised_film_carries_rating_year_and_genre() {
    let source = MockSource::new(Answer::Bodies(CATALOGUE, DETAIL));
    let enricher = metadata_enricher(Arc::clone(&source));
    let fields = enricher
        .enrich("https://indexer.example/dl/abcdef", Some(FILM), None)
        .await
        .expect("a lookup that answered");
    assert_eq!(field(&fields, "metadata.title"), Some("Blade Runner 2049"));
    assert_eq!(field(&fields, "metadata.year"), Some("2017"));
    assert_eq!(field(&fields, "metadata.rating"), Some("8.0"));
    assert_eq!(
        field(&fields, "metadata.genre"),
        Some("Action, Drama, Mystery")
    );
    // The runtime is not in a catalogue entry, so it costs the one extra request — to the
    // same host, with the identifier that host just handed back.
    assert_eq!(field(&fields, "metadata.runtime"), Some("164 min"));
    let requests = source.requests();
    assert_eq!(requests.len(), 2, "{requests:?}");
    assert!(
        requests
            .iter()
            .all(|url| url.starts_with("https://v3-cinemeta.strem.io/")),
        "{requests:?}"
    );
    // The year out of the file name decides which of the two entries the row describes.
    assert!(field(&fields, "metadata.title") != Some("Blade Runner"));
}

#[tokio::test]
async fn an_episode_carries_its_season_and_number_rather_than_a_film_title() {
    let source = MockSource::new(Answer::Bodies(SERIES_CATALOGUE, DETAIL));
    let enricher = metadata_enricher(Arc::clone(&source));
    let fields = enricher
        .enrich(
            "https://indexer.example/dl/abcdef",
            Some("Breaking.Bad.S02E05.1080p.WEB-DL.x264-GRP.mkv"),
            None,
        )
        .await
        .expect("a lookup that answered");
    assert_eq!(field(&fields, "metadata.series"), Some("Breaking Bad"));
    assert_eq!(field(&fields, "metadata.season"), Some("2"));
    assert_eq!(field(&fields, "metadata.episode"), Some("5"));
    assert_eq!(field(&fields, "metadata.title"), None, "not a film");
    // The series catalogue was asked, not the film one.
    assert!(
        source
            .requests()
            .iter()
            .all(|url| url.contains("/catalog/series/")),
        "{:?}",
        source.requests()
    );
}

#[tokio::test]
async fn a_hit_that_is_not_the_release_searched_for_adds_nothing_the_source_said() {
    // RD-108-14, at the boundary that matters: the built component, asked about the reported
    // release, against the answer the source really gives for it. The row may carry what the
    // file name itself says and not one word the source said, because none of it belongs to
    // this release.
    let source = MockSource::new(Answer::Bodies(WRONG_SERIES, DETAIL));
    let enricher = metadata_enricher(Arc::clone(&source));
    let fields = enricher
        .enrich("https://indexer.example/dl/abcdef", Some(APOLLO), None)
        .await
        .expect("the check stays successful");
    assert_eq!(field(&fields, "metadata.series"), Some("Apollo Has Fallen"));
    assert_eq!(field(&fields, "metadata.season"), Some("2"));
    assert_eq!(field(&fields, "metadata.episode"), Some("2"));
    for name in [
        "metadata.year",
        "metadata.rating",
        "metadata.genre",
        "metadata.runtime",
        "metadata.title",
    ] {
        assert_eq!(field(&fields, name), None, "{name} came from the wrong hit");
    }
    // An uncertain hit is not a second chance: the detail is never asked for, because there is
    // no identifier this plugin is willing to believe.
    assert_eq!(source.requests().len(), 1, "{:?}", source.requests());
}

#[tokio::test]
async fn an_episode_without_a_season_is_an_episode_and_not_a_film() {
    let source = MockSource::new(Answer::Bodies(WRONG_FILM, DETAIL));
    let enricher = metadata_enricher(Arc::clone(&source));
    let fields = enricher
        .enrich("https://indexer.example/dl/abcdef", Some(DAIMA), None)
        .await
        .expect("the check stays successful");
    // The series catalogue is the one asked, and the season stays unsaid because the name
    // never said it.
    assert!(
        source
            .requests()
            .iter()
            .all(|url| url.contains("/catalog/series/")),
        "{:?}",
        source.requests()
    );
    assert_eq!(field(&fields, "metadata.series"), Some("Dragon Ball DAIMA"));
    assert_eq!(field(&fields, "metadata.episode"), Some("15"));
    assert_eq!(
        field(&fields, "metadata.season"),
        None,
        "no season is known"
    );
    assert_eq!(field(&fields, "metadata.title"), None, "not a film");
    // And the film the old reading came back with reaches the row nowhere.
    assert_eq!(field(&fields, "metadata.year"), None);
    assert_eq!(field(&fields, "metadata.rating"), None);
}

#[tokio::test]
async fn a_source_that_does_not_answer_leaves_the_row_alone_and_the_check_successful() {
    let source = MockSource::new(Answer::Silence);
    let enricher = metadata_enricher(Arc::clone(&source));
    // A film: nothing came back, so nothing is shown — and, above all, the call is `Ok`. An
    // enricher reporting a failure would turn a link the online check resolved perfectly well
    // into one somebody has to think about.
    let fields = enricher
        .enrich("https://indexer.example/dl/abcdef", Some(FILM), None)
        .await
        .expect("the check stays successful");
    assert!(fields.is_empty(), "{fields:?}");
    // An episode: what the file name itself said survives the source being unreachable,
    // because it never depended on it.
    let fields = enricher
        .enrich(
            "https://indexer.example/dl/abcdef",
            Some("Breaking.Bad.S02E05.1080p.WEB-DL.x264-GRP.mkv"),
            None,
        )
        .await
        .expect("the check stays successful");
    assert_eq!(field(&fields, "metadata.series"), Some("Breaking Bad"));
    assert_eq!(field(&fields, "metadata.season"), Some("2"));
    assert_eq!(field(&fields, "metadata.episode"), Some("5"));
}

#[tokio::test]
async fn a_source_answering_nonsense_is_the_same_as_no_answer() {
    let source = MockSource::new(Answer::Bodies("<html>502 Bad Gateway</html>", "null"));
    let enricher = metadata_enricher(Arc::clone(&source));
    let fields = enricher
        .enrich("https://indexer.example/dl/abcdef", Some(FILM), None)
        .await
        .expect("the check stays successful");
    assert!(fields.is_empty(), "{fields:?}");
}

#[tokio::test]
async fn nothing_this_plugin_offers_can_replace_a_field_the_core_resolved() {
    // The field names are checked here rather than only in the adapter's own tests, because
    // a name that collides is dropped silently from the plugin's point of view: the chip
    // would simply never appear, with nothing in the row to say why.
    let source = MockSource::new(Answer::Bodies(CATALOGUE, DETAIL));
    let enricher = metadata_enricher(Arc::clone(&source));
    let fields = enricher
        .enrich("https://indexer.example/dl/abcdef", Some(FILM), None)
        .await
        .expect("a lookup that answered");
    assert!(!fields.is_empty());
    for (name, value) in &fields {
        assert!(name.starts_with("metadata."), "{name}");
        assert!(value.chars().count() <= 512, "{name} is too long to show");
    }
    assert!(fields.len() <= 32, "the host keeps at most 32");
}
