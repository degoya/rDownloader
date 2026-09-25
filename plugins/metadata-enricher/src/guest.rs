//! The component: a link in, a few fields about the film or episode out — or nothing at all.
//!
//! Two rules shape everything here. Nothing is a failure: a source that does not answer, is
//! slow, or answers with nonsense leaves the row exactly as the online check left it, because
//! an enricher must never turn a link that resolved perfectly well into one somebody has to
//! think about. And no request is made until [`release::parse`] has said there is a film or an
//! episode to ask about, which for most links it does not.
#![allow(unsafe_code)] // Generated canonical-ABI exports contain the only unsafe code here.

wit_bindgen::generate!({
    path: "../../crates/rd-plugin-api/wit",
    world: "enricher-plugin",
});

use exports::rdownloader::plugin::enricher::{EnrichField, EnrichSubject, Guest};
use rdownloader::plugin::{http, types::Failure};

use crate::{
    lookup::{self, Kind},
    release::{self, ReleaseName},
};

struct Component;

impl Guest for Component {
    fn enrich(subject: EnrichSubject) -> Result<Vec<EnrichField>, Failure> {
        // Always `Ok`. There is no outcome of this plugin that should cost a candidate its
        // green state, and the host logs an `Err` as a plugin that misbehaved.
        Ok(fields_for(&subject)
            .into_iter()
            .map(|(name, value)| EnrichField { name, value })
            .collect())
    }
}

/// Everything this plugin has to say about one link.
fn fields_for(subject: &EnrichSubject) -> Vec<(String, String)> {
    // The file name is what carries the content; the URL is only a fallback for a link whose
    // name the check did not resolve, and it is read the same way.
    let name = subject.file_name.as_deref().unwrap_or(&subject.url);
    // The decision, made here and made for free. Most links end on this line.
    let Some(parsed) = release::parse(name) else {
        return Vec::new();
    };
    let kind = if parsed.is_series() {
        Kind::Series
    } else {
        Kind::Movie
    };
    let mut found = ask(&parsed, kind);
    let mut fields = Vec::new();
    let named = take(&mut found, kind.name_field());
    match kind {
        // An episode's season and number come from the name and are worth a chip whether or
        // not the source answered: they are what says this is not a film.
        Kind::Series => {
            fields.push((
                lookup::FIELD_SERIES.to_owned(),
                named.unwrap_or_else(|| parsed.title.clone()),
            ));
            if let Some(season) = parsed.season {
                fields.push((lookup::FIELD_SEASON.to_owned(), season.to_string()));
            }
            if let Some(episode) = parsed.episode {
                fields.push((lookup::FIELD_EPISODE.to_owned(), episode.to_string()));
            }
        }
        // A film's title is already in the file name, so it earns a chip only when the source
        // answered and the chip therefore says which film this was taken to be.
        Kind::Movie => {
            if let Some(title) = named {
                fields.push((lookup::FIELD_TITLE.to_owned(), title));
            }
        }
    }
    fields.extend(found);
    fields
}

/// Asks the source, and gives up quietly at every step where it can.
fn ask(parsed: &ReleaseName, kind: Kind) -> Vec<(String, String)> {
    let Some(url) = lookup::catalogue_url(kind, &parsed.title) else {
        return Vec::new();
    };
    let Some(body) = get(&url) else {
        return Vec::new();
    };
    // The title goes in as well as out: the answer is only used when its own name reads like
    // the one that was searched for.
    let Some(found) = lookup::from_catalogue(&body, kind, &parsed.title, parsed.year) else {
        return Vec::new();
    };
    let mut fields = found.fields;
    // A second request only when the first answer left out something a person would read, and
    // only to the same host, with an identifier that host itself just handed back.
    if lookup::wants_detail(&fields)
        && let Some(id) = found.id
        && let Some(detail) = lookup::meta_url(kind, &id)
        && let Some(body) = get(&detail)
    {
        lookup::merge(&mut fields, lookup::from_meta(&body));
    }
    fields
}

/// One GET, and nothing at all on anything other than a plain success.
fn get(url: &str) -> Option<String> {
    let response = http::http_request("GET", url, &[], &[], &[]).ok()?;
    if !(200..300).contains(&response.status) {
        return None;
    }
    Some(String::from_utf8_lossy(&response.body).into_owned())
}

/// Removes the field called `name` and returns its value.
fn take(fields: &mut Vec<(String, String)>, name: &str) -> Option<String> {
    let at = fields.iter().position(|(field, _)| field == name)?;
    Some(fields.remove(at).1)
}

export!(Component);
