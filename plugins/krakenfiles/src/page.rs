//! Pure parsing of KrakenFiles URLs, pages and answers. Nothing here talks to a host.
//!
//! Every marker below was read off the pages measured on 2026-09-21 and kept as fixtures
//! under `tests/fixtures/` (RD-103-08): the file page's `<form ... id="dl-form">` with its
//! hidden `token`, the `data-file-hash` attribute, the `cf-turnstile` widget, the file name in
//! `class="coin-name"`, the "File size" row, and the 404 page's "File has been deleted or
//! never existed" notice.

use url::Url;

/// The apex domain every request goes to; `www.` is normalised away.
pub(crate) const PRIMARY_DOMAIN: &str = "krakenfiles.com";

/// Hosts a KrakenFiles link may carry.
const MATCH_HOSTS: &[&str] = &["krakenfiles.com", "www.krakenfiles.com"];

/// Hosts the site may hand a direct link out on, as the manifest's `download_domains` allow
/// them. Kept in step with `manifest.toml` by `resolver::tests`.
pub(crate) const DOWNLOAD_HOSTS: &[&str] =
    &["krakenfiles.com", "*.krakenfiles.com", "*.krakencloud.net"];

/// The notice on the site's own 404 page; the page answers 404 too, but a body can carry the
/// notice under any status the site chooses tomorrow.
const UNAVAILABLE_NOTICE: &str = "File has been deleted or never existed";

/// The file id of a supported link, lowercased: the site answers the same file for any case
/// (measured on the page and on `/json/`), so the lowercased id is the link's identity.
///
/// Supported are `/view/<id>/file.html` (JDownloader's pattern) and `/embed-video/<id>`, the
/// site's own player, which embeds the same file. Short forms such as `/view/<id>` or `/<id>`
/// answer 404 and are refused.
#[must_use]
pub(crate) fn file_id(url: &Url) -> Option<String> {
    let host = url.host_str()?.to_ascii_lowercase();
    if !MATCH_HOSTS.contains(&host.as_str()) {
        return None;
    }
    let segments: Vec<&str> = url
        .path_segments()?
        .filter(|segment| !segment.is_empty())
        .collect();
    let id = match segments.as_slice() {
        ["view", id, "file.html"] | ["embed-video", id] => *id,
        _ => return None,
    };
    if id.is_empty()
        || !id
            .chars()
            .all(|character| character.is_ascii_alphanumeric())
    {
        return None;
    }
    Some(id.to_ascii_lowercase())
}

/// The file page, on the apex domain.
#[must_use]
pub(crate) fn file_page_url(id: &str) -> String {
    format!("https://{PRIMARY_DOMAIN}/view/{id}/file.html")
}

/// The metadata endpoint behind the embed player: no token, no captcha, `[]` for a file that
/// is gone.
#[must_use]
pub(crate) fn json_url(id: &str) -> String {
    format!("https://{PRIMARY_DOMAIN}/json/{id}")
}

/// What the download form carries, as the flow needs it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DownloadForm {
    /// The `POST` target, absolute.
    pub(crate) action: String,
    /// The page's own hidden `token`.
    pub(crate) token: String,
    /// The `data-file-hash`, sent as the `hash` header the way pyLoad does.
    pub(crate) hash: String,
    /// The Turnstile widget's site key.
    pub(crate) site_key: String,
}

/// Reads the download form off the file page, or says what is missing.
///
/// # Errors
///
/// The diagnosis to report under `krakenfiles.page_layout_changed`: which of the four parts
/// the page lacks, or - when there is no form at all - what page arrived instead.
pub(crate) fn download_form(html: &str) -> Result<DownloadForm, String> {
    let Some(form) = form_html(html) else {
        return Err(format!("no download form on the page: {}", diagnose(html)));
    };
    let action = attribute(form, "action").ok_or("the download form has no action")?;
    let token = input_value(form, "token").ok_or("the download form has no token")?;
    let hash = attribute(html, "data-file-hash").ok_or("the page carries no data-file-hash")?;
    let site_key = match xfs_common::free::widget_marker(form) {
        Some(marker) if marker.kind == xfs_common::free::WidgetKind::Turnstile => marker.site_key,
        Some(marker) => {
            return Err(format!(
                "the download form asks for {} instead of Cloudflare Turnstile",
                marker.kind.display_name()
            ));
        }
        None => return Err("the download form has no Turnstile widget".to_owned()),
    };
    Ok(DownloadForm {
        action: absolute(&action),
        token,
        hash,
        site_key,
    })
}

/// Whether the page is the site's "gone" notice.
#[must_use]
pub(crate) fn is_file_unavailable(html: &str) -> bool {
    html.contains(UNAVAILABLE_NOTICE)
}

/// The file name as the page shows it: the `coin-name` heading, else the title without the
/// site's suffix.
#[must_use]
pub(crate) fn file_name(html: &str) -> Option<String> {
    let from_heading = html
        .find("class=\"coin-name\"")
        .and_then(|at| {
            html[at..]
                .find("<h5>")
                .map(|offset| at + offset + "<h5>".len())
        })
        .and_then(|start| {
            html[start..]
                .find('<')
                .map(|end| html[start..start + end].trim())
        })
        .filter(|name| !name.is_empty())
        .map(str::to_owned);
    from_heading.or_else(|| {
        let title = element_text(html, "<title>")?;
        let name = title
            .strip_suffix("Krakenfiles.com")
            .unwrap_or(&title)
            .trim_end_matches([' ', '-'])
            .trim();
        (!name.is_empty()).then(|| name.to_owned())
    })
}

/// A size the site prints, such as `4.90 MB`, in bytes. Approximate by nature - the site
/// rounds to two decimals - and read with 1024-based units, as JDownloader's `SizeFormatter`
/// reads the same string.
#[must_use]
pub(crate) fn parse_size(text: &str) -> Option<u64> {
    let text = text.trim();
    let split = text
        .find(|character: char| character.is_ascii_alphabetic())
        .unwrap_or(text.len());
    let number: f64 = text[..split].trim().parse().ok()?;
    let unit = text[split..].trim().to_ascii_uppercase();
    let factor: f64 = match unit.as_str() {
        "" | "B" | "BYTES" => 1.0,
        "KB" | "KIB" => 1024.0,
        "MB" | "MIB" => 1024.0 * 1024.0,
        "GB" | "GIB" => 1024.0 * 1024.0 * 1024.0,
        "TB" | "TIB" => 1024.0 * 1024.0 * 1024.0 * 1024.0,
        _ => return None,
    };
    let bytes = number * factor;
    if !bytes.is_finite() || bytes < 0.0 {
        return None;
    }
    // Rounded and bounded: the cast saturates, and nothing above u64 is a file size.
    Some(bytes.round() as u64)
}

/// The total length a `Content-Range: bytes 0-0/<total>` header states.
#[must_use]
pub(crate) fn content_range_total(value: &str) -> Option<u64> {
    value.rsplit('/').next()?.trim().parse().ok()
}

/// Explains why a page came back instead of what was expected: the page's own alert if it
/// carries one, else its title.
#[must_use]
pub(crate) fn diagnose(html: &str) -> String {
    if is_file_unavailable(html) {
        return format!("page message: {UNAVAILABLE_NOTICE}");
    }
    match element_text(html, "<title>").filter(|text| !text.is_empty()) {
        Some(title) => format!("page \"{title}\""),
        None => "the response page has no title".to_owned(),
    }
}

/// Encodes form fields as `application/x-www-form-urlencoded`.
#[must_use]
pub(crate) fn encode_form(fields: &[(String, String)]) -> Vec<u8> {
    xfs_common::page::encode_form(fields)
}

/// The `<form ... id="dl-form">` element, from its opening tag to its closing tag.
fn form_html(html: &str) -> Option<&str> {
    let marker = html.find("id=\"dl-form\"")?;
    let start = html[..marker].rfind("<form")?;
    let end = html[marker..].find("</form>")? + marker + "</form>".len();
    Some(&html[start..end])
}

/// The first `name="<attribute>"` value in `html`, double- or single-quoted.
fn attribute(html: &str, name: &str) -> Option<String> {
    let needle = format!("{name}=");
    let at = html.find(&needle)? + needle.len();
    let rest = &html[at..];
    let quote = rest.chars().next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    let value = &rest[1..];
    let end = value.find(quote)?;
    Some(value[..end].to_owned()).filter(|value| !value.is_empty())
}

/// The `value` of the `<input>` named `name`, wherever in the tag the two attributes stand.
fn input_value(html: &str, name: &str) -> Option<String> {
    let needle = format!("name=\"{name}\"");
    html.match_indices("<input").find_map(|(at, _)| {
        let tag = &html[at..at + html[at..].find('>')?];
        tag.contains(&needle)
            .then(|| attribute(tag, "value"))
            .flatten()
    })
}

/// A form action as the site writes it (`/download/<hash>`), made absolute on the apex domain.
fn absolute(action: &str) -> String {
    if action.starts_with("http://") || action.starts_with("https://") {
        action.to_owned()
    } else {
        format!(
            "https://{PRIMARY_DOMAIN}/{}",
            action.trim_start_matches('/')
        )
    }
}

/// Text content following `marker` up to the next `<`, whitespace collapsed and capped.
fn element_text(html: &str, marker: &str) -> Option<String> {
    let start = html.find(marker)? + marker.len();
    let rest = &html[start..];
    let text = &rest[..rest.find('<').unwrap_or(rest.len())];
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    Some(collapsed.chars().take(160).collect())
}

#[cfg(test)]
mod tests {
    use super::{
        content_range_total, diagnose, download_form, file_id, file_name, is_file_unavailable,
        parse_size,
    };
    use url::Url;

    const FILE_PAGE: &str = include_str!("../tests/fixtures/file-page-2026-09-21.html");
    const ERROR_PAGE: &str = include_str!("../tests/fixtures/error-page-2026-09-21.html");

    fn id_of(link: &str) -> Option<String> {
        file_id(&Url::parse(link).expect("url"))
    }

    #[test]
    fn supported_links_are_recognised_and_normalised() {
        for link in [
            "https://krakenfiles.com/view/DP3nGKJNsX/file.html",
            "https://www.krakenfiles.com/view/DP3nGKJNsX/file.html",
            "http://krakenfiles.com/view/DP3nGKJNsX/file.html",
            "https://krakenfiles.com/view/DP3NGKJNSX/file.html",
            "https://KRAKENFILES.com/view/dp3ngkjnsx/file.html",
            "https://krakenfiles.com/embed-video/DP3nGKJNsX",
            "https://krakenfiles.com/embed-video/DP3nGKJNsX/",
        ] {
            assert_eq!(id_of(link).as_deref(), Some("dp3ngkjnsx"), "{link}");
        }
    }

    #[test]
    fn short_forms_and_foreign_links_are_refused() {
        for link in [
            "https://krakenfiles.com/view/DP3nGKJNsX",
            "https://krakenfiles.com/view/DP3nGKJNsX/",
            "https://krakenfiles.com/DP3nGKJNsX",
            "https://krakenfiles.com/file/DP3nGKJNsX",
            "https://krakenfiles.com/download/DP3nGKJNsX",
            "https://krakenfiles.com/folder/DP3nGKJNsX",
            "https://krakenfiles.com/view/DP3nGKJNsX/other.html",
            "https://krakenfiles.com/view/../file.html",
            "https://krakenfiles.com/json/DP3nGKJNsX",
            "https://krakenfiles.net/view/DP3nGKJNsX/file.html",
            "https://example.com/view/DP3nGKJNsX/file.html",
            "https://hs3.krakenfiles.com/view/DP3nGKJNsX/file.html",
        ] {
            assert_eq!(id_of(link), None, "{link}");
        }
    }

    #[test]
    fn the_measured_file_page_yields_the_whole_form() {
        let form = download_form(FILE_PAGE).expect("the fixture carries the form");
        assert_eq!(form.action, "https://krakenfiles.com/download/DP3nGKJNsX");
        assert_eq!(form.token, "dl-token-redacted-0000000000000000000000");
        assert_eq!(form.hash, "DP3nGKJNsX");
        assert_eq!(form.site_key, "0x4AAAAAAB4S-Cq-7quNHQy8");
        assert_eq!(
            file_name(FILE_PAGE).as_deref(),
            Some("EldenRing_Fix_Repair_Steam_Generic.rar")
        );
        assert!(!is_file_unavailable(FILE_PAGE));
    }

    #[test]
    fn a_form_missing_a_part_names_the_part() {
        let without_token = FILE_PAGE.replace("name=\"token\"", "name=\"tkn\"");
        assert_eq!(
            download_form(&without_token).expect_err("no token"),
            "the download form has no token"
        );
        let without_hash = FILE_PAGE.replace("data-file-hash", "data-hash");
        assert_eq!(
            download_form(&without_hash).expect_err("no hash"),
            "the page carries no data-file-hash"
        );
        let without_widget = FILE_PAGE.replace("cf-turnstile", "cf-nothing");
        assert_eq!(
            download_form(&without_widget).expect_err("no widget"),
            "the download form has no Turnstile widget"
        );
        let recaptcha = FILE_PAGE.replace("class=\"cf-turnstile mt-2\"", "class=\"g-recaptcha\"");
        assert_eq!(
            download_form(&recaptcha).expect_err("wrong widget"),
            "the download form asks for reCAPTCHA instead of Cloudflare Turnstile"
        );
    }

    #[test]
    fn the_error_page_is_recognised_and_diagnosed() {
        assert!(is_file_unavailable(ERROR_PAGE));
        assert_eq!(
            download_form(ERROR_PAGE).expect_err("no form"),
            "no download form on the page: page message: File has been deleted or never existed"
        );
        assert_eq!(
            diagnose("<html><head><title>Maintenance - Krakenfiles.com</title></head></html>"),
            "page \"Maintenance - Krakenfiles.com\""
        );
        assert_eq!(diagnose("<html></html>"), "the response page has no title");
    }

    #[test]
    fn the_title_is_the_fallback_file_name() {
        assert_eq!(
            file_name("<title>release.rar - Krakenfiles.com</title>").as_deref(),
            Some("release.rar")
        );
        assert_eq!(file_name("<title> - Krakenfiles.com</title>"), None);
        assert_eq!(file_name("<p>nothing</p>"), None);
    }

    #[test]
    fn printed_sizes_are_read_in_1024_based_units() {
        assert_eq!(parse_size("4.90 MB"), Some(5_138_022));
        assert_eq!(parse_size("512 B"), Some(512));
        assert_eq!(parse_size("1.5 KB"), Some(1536));
        assert_eq!(parse_size("2 GB"), Some(2_147_483_648));
        assert_eq!(parse_size("0.5TB"), Some(549_755_813_888));
        assert_eq!(parse_size("many MB"), None);
        assert_eq!(parse_size("4.90 PB"), None);
        assert_eq!(parse_size(""), None);
    }

    #[test]
    fn the_content_range_total_is_the_file_size() {
        assert_eq!(content_range_total("bytes 0-0/5138022"), Some(5_138_022));
        assert_eq!(content_range_total("bytes 0-0/*"), None);
        assert_eq!(content_range_total("bytes"), None);
    }
}
