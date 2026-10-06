//! The account-less XFS download flow itself, driven once (RD-1120-10, PL-2).
//!
//! Mirrors JD's `XFileSharingProBasic.doFree`: fetch the file page, post the `download1` form in
//! free mode, solve the captcha the answer asks for, wait out the countdown, post `download2`,
//! and take the direct link from what comes back. Every page is checked for a dead end first —
//! an IP limit above all, because hitting one means no amount of waiting or captcha solving will
//! help until it expires.
//!
//! `ddownload`, `katfile`, `filejoker` and `xfs-generic` each carried this flow, 76 to 89 per
//! cent alike. What really differs is a [`FreeFlow`] field now: KatFile browses a link on
//! today's main domain ([`FreeFlow::rewrite_host`]), DDownload clears its adblock field
//! ([`FreeFlow::free_fields`]) and starts at the second step when its file page carries that form
//! directly ([`FreeFlow::download2_on_file_page`], RD-108-28), FileJoker reads more dead ends off
//! a page ([`FreeFlow::page_check`]), and each site has its own countdown, captcha scan, link
//! domains and referer. KatFile's retry loop returned the rejection from inside where the others
//! left the loop and returned it after: the same failure, so there is one loop.

use plugin_common::{
    Failure, FailureKind, Header, HttpResponse, PluginHost, Resolved, file_name_from_disposition,
};
use url::Url;

use super::{FreeWords, WidgetMarker, challenge_for, is_wrong_captcha, with_captcha_token};
use crate::glue::{coded, is_html, range_probe};
use crate::site::second_path_segment;
use crate::standard::{download1_form, download2_form};

/// A free submission built from a form's raw fields.
pub type FreeFields = fn(&[(String, String)]) -> Vec<(String, String)>;

/// A plugin's own dead-end check for a page.
pub type PageCheck = fn(&str) -> Result<(), Failure>;

/// One plugin's free flow: its words and the places its site departs from the script.
#[derive(Clone, Copy)]
pub struct FreeFlow {
    /// The codes the flow's dead ends are reported under.
    pub words: FreeWords,
    /// The plugin's `invalid_url` code, for a direct link that does not parse.
    pub invalid_url: &'static str,
    /// The plugin's `captcha_rejected` words, for a second wrong answer.
    pub captcha_rejected: (&'static str, &'static str),
    /// Rewrites the link before the file page is fetched; `None` fetches it as given.
    pub rewrite_host: Option<fn(&str) -> String>,
    /// Whether a file page carrying the `download2` form but no `download1` starts the flow at
    /// the second step, rather than being a page this flow does not know.
    pub download2_on_file_page: bool,
    /// The free submission for either step's raw fields.
    pub free_fields: FreeFields,
    /// The plugin's own dead-end check for every page; `None` checks the IP limit alone
    /// ([`FreeWords::free_page_failure`]).
    pub page_check: Option<PageCheck>,
    /// The captcha widget a page asks for.
    pub widget_marker: fn(&str) -> Option<WidgetMarker>,
    /// Seconds a page asks the visitor to wait before the form may be posted.
    pub wait_seconds: fn(&str) -> Option<u64>,
    /// The direct link on the last page: its HTML, the link being resolved and the hints (the
    /// link's file name and code) a matching link carries.
    pub direct_link: fn(&str, &Url, &[&str]) -> Option<String>,
    /// The `Referer` the transfer carries, for the link being resolved: the hoster rejects a
    /// direct link fetched without the page it came from.
    pub referer: fn(&Url) -> Header,
}

/// What a response turned out to be.
enum Answer {
    /// Not a page: the file itself, served in answer.
    File(HttpResponse),
    /// A page, with its text, that passed the dead-end check.
    Page(HttpResponse, String),
}

impl FreeFlow {
    /// Runs the flow for `url` (`parsed` is the same link, `code` its file code) and turns its
    /// result into a transfer.
    ///
    /// # Errors
    ///
    /// The host's failure, a classified status, a dead end the page explains, or
    /// `captcha_rejected` after a second wrong answer.
    pub async fn resolve<H: PluginHost>(
        &self,
        host: &H,
        url: &str,
        parsed: &Url,
        code: &str,
    ) -> Result<Resolved, Failure> {
        let url_name = second_path_segment(parsed);
        let transfer = self
            .transfer(host, url, parsed, code, url_name.as_deref())
            .await?;
        let disposition = transfer.header("content-disposition").map(str::to_owned);
        if disposition.is_none() && is_html(&transfer) {
            return Err(self.words.no_free_link(&transfer.text()));
        }
        Ok(Resolved {
            url: transfer.final_url,
            file_name: disposition
                .as_deref()
                .and_then(file_name_from_disposition)
                .or(url_name),
            size: None,
            headers: vec![(self.referer)(parsed)],
            checksum: None,
        })
    }

    /// The two-form free flow, up to and including the direct link's range probe.
    async fn transfer<H: PluginHost>(
        &self,
        host: &H,
        url: &str,
        parsed: &Url,
        code: &str,
        url_name: Option<&str>,
    ) -> Result<HttpResponse, Failure> {
        let page_url = self
            .rewrite_host
            .map_or_else(|| url.to_owned(), |rewrite| rewrite(url));
        let page_response = host.http(range_probe(page_url)).await?;
        self.ensure_http_status(&page_response)?;
        // A hotlink is possible: some XFS installations serve the file straight away.
        let (page_response, body) = match self.read(page_response)? {
            Answer::File(file) => return Ok(file),
            Answer::Page(response, body) => (response, body),
        };
        let (posted, posted_body) = match download1_form(&body) {
            Some(step_one) => {
                let fields = (self.free_fields)(&step_one);
                let posted = self
                    .words
                    .post_form(host, &page_response.final_url, &fields)
                    .await?;
                match self.read(posted)? {
                    Answer::File(file) => return Ok(file),
                    Answer::Page(response, body) => (response, body),
                }
            }
            None if self.download2_on_file_page && download2_form(&body).is_some() => {
                (page_response, body)
            }
            None => return Err(self.words.no_free_form(&body)),
        };
        let final_page = self.submit_download2(host, &posted, &posted_body).await?;
        let final_body = match self.read(final_page)? {
            Answer::File(file) => return Ok(file),
            Answer::Page(_, body) => body,
        };
        let hints: Vec<&str> = url_name.into_iter().chain([code]).collect();
        let Some(link) = (self.direct_link)(&final_body, parsed, &hints) else {
            return Err(self.words.no_free_link(&final_body));
        };
        Url::parse(&link).map_err(|error| {
            Failure::from(plugin_common::failure::invalid_url(
                self.invalid_url,
                &error,
            ))
        })?;
        let transfer = host.http(range_probe(link)).await?;
        self.ensure_http_status(&transfer)?;
        Ok(transfer)
    }

    /// Solves the captcha, waits out the countdown and posts `download2`. A rejected captcha is
    /// retried once with a fresh challenge, the way JD's `download2` loop retries it.
    async fn submit_download2<H: PluginHost>(
        &self,
        host: &H,
        posted: &HttpResponse,
        posted_body: &str,
    ) -> Result<HttpResponse, Failure> {
        let mut fields =
            second_form(posted_body).ok_or_else(|| self.words.no_free_form(posted_body))?;
        let mut attempt_body = posted_body.to_owned();
        for attempt in 0..2 {
            let submitted = self
                .submission(host, &fields, &attempt_body, &posted.final_url)
                .await?;
            let response = self
                .words
                .post_form(host, &posted.final_url, &submitted)
                .await?;
            let (response, body) = match self.read(response)? {
                Answer::File(file) => return Ok(file),
                Answer::Page(response, body) => (response, body),
            };
            if !is_wrong_captcha(&body) {
                return Ok(response);
            }
            if attempt == 1 {
                break;
            }
            // Retry with whatever the rejection page now asks for.
            fields = second_form(&body).ok_or_else(|| self.words.no_free_form(&body))?;
            attempt_body = body;
        }
        Err(coded(FailureKind::CaptchaFailed, self.captcha_rejected))
    }

    /// The submission for one attempt: the free fields, the captcha the page asks for answered,
    /// and the countdown waited out. JD solves the captcha first and then waits out the
    /// remainder, so the token is as fresh as possible when the form is finally posted.
    async fn submission<H: PluginHost>(
        &self,
        host: &H,
        fields: &[(String, String)],
        page: &str,
        page_url: &str,
    ) -> Result<Vec<(String, String)>, Failure> {
        let mut submitted = (self.free_fields)(fields);
        if let Some(marker) = (self.widget_marker)(page) {
            let solution = host.solve_captcha(challenge_for(&marker, page_url)).await?;
            submitted = with_captcha_token(&submitted, marker.kind, &solution.token);
        }
        if let Some(seconds) = (self.wait_seconds)(page)
            && let Ok(seconds) = u32::try_from(seconds)
        {
            host.wait(seconds).await?;
        }
        Ok(submitted)
    }

    /// A response the file itself, or a page that passed the dead-end check.
    fn read(&self, response: HttpResponse) -> Result<Answer, Failure> {
        if !is_html(&response) {
            return Ok(Answer::File(response));
        }
        let body = response.text().into_owned();
        match self.page_check {
            Some(check) => check(&body)?,
            None => self.words.free_page_failure(&body)?,
        }
        Ok(Answer::Page(response, body))
    }

    fn ensure_http_status(&self, response: &HttpResponse) -> Result<(), Failure> {
        let error = self.words.http_error;
        crate::glue::ensure_http_status(response, error.code, error.text)
    }
}

/// The form the second step posts: a page may ask for `download1` again before `download2`.
fn second_form(html: &str) -> Option<Vec<(String, String)>> {
    download1_form(html).or_else(|| download2_form(html))
}
