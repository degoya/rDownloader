//! One run: the state a rule accumulates, and every request it is allowed to make.
//!
//! Everything that touches the network goes through [`Run::issue`], and that is the point of
//! the module: the four limits, the two bolts and the cycle set are checked in one place, so
//! a new step kind cannot be written that forgets one of them.

use std::{
    collections::{BTreeMap, BTreeSet},
    net::IpAddr,
};

use url::Url;

use super::{
    Limits, Ports,
    error::RunError,
    guard::{host_allowed, is_public, literal_address},
    ports::{FetchFailure, FetchRequest, FetchResponse, Method},
    value::{ADDRESS_VARIABLE, PAGE_URL_VARIABLE, Variables},
};
use crate::format::Rule;

/// The state of one rule run.
pub(crate) struct Run<'a> {
    pub(crate) ports: Ports<'a>,
    pub(crate) limits: Limits,
    pub(crate) rule: &'a Rule,
    pub(crate) origin_host: String,
    pub(crate) address: Url,
    pub(crate) variables: Variables,
    /// Every address already requested in this run. A rule that comes back to one has found
    /// a cycle, and a cycle ends here instead of spending the time budget.
    seen: BTreeSet<String>,
    pages: u32,
    /// How many requests deep the page currently held sits from the address the run started
    /// with. The starting address is depth 0.
    pub(crate) depth: u32,
    started: std::time::Duration,
}

impl<'a> Run<'a> {
    pub(crate) fn new(
        ports: Ports<'a>,
        limits: Limits,
        rule: &'a Rule,
        address: Url,
        origin_host: String,
    ) -> Self {
        let mut variables = Variables::default();
        variables.set(ADDRESS_VARIABLE, address.to_string());
        variables.set(PAGE_URL_VARIABLE, address.to_string());
        Self {
            ports,
            limits,
            rule,
            origin_host,
            address,
            variables,
            seen: BTreeSet::new(),
            pages: 0,
            depth: 0,
            started: ports.clock.elapsed(),
        }
    }

    /// How many pages the run fetched, for the caller's log.
    pub(crate) fn pages(&self) -> u32 {
        self.pages
    }

    /// Refuses once the run has spent its budget. Checked before every request and before
    /// every step, so a rule with many cheap steps cannot outrun it either.
    pub(crate) fn check_time(&self) -> Result<(), RunError> {
        if self.ports.clock.elapsed().saturating_sub(self.started) > self.limits.max_total_time {
            return Err(RunError::LimitTime(self.limits.max_total_time.as_secs()));
        }
        Ok(())
    }

    /// The two bolts, in the order that leaks least: a host outside the rule's own `match` is
    /// refused before anything is resolved, so a rule cannot be used to ask this installation's
    /// resolver about arbitrary names.
    ///
    /// Returns the addresses it checked, which then travel with the request. Resolving once
    /// and connecting to that answer is the whole point: a second lookup inside the adapter
    /// is a second answer, and a record with a time-to-live of zero is free to make the two
    /// differ. Empty when the host is already a literal address.
    async fn guard(&self, url: &Url) -> Result<Vec<IpAddr>, RunError> {
        if !host_allowed(self.rule, &self.origin_host, url) {
            return Err(RunError::TargetNotAllowed {
                url: url.to_string(),
            });
        }
        let Some(host) = url.host_str() else {
            return Err(RunError::TargetNotAllowed {
                url: url.to_string(),
            });
        };
        if let Some(address) = literal_address(url) {
            return if is_public(address) {
                Ok(Vec::new())
            } else {
                Err(RunError::AddressNotPublic {
                    host: host.to_owned(),
                    address,
                })
            };
        }
        let addresses =
            self.ports
                .resolver
                .resolve(host)
                .await
                .map_err(|reason| RunError::PageDead {
                    url: url.to_string(),
                    reason,
                })?;
        if addresses.is_empty() {
            return Err(RunError::PageDead {
                url: url.to_string(),
                reason: "the name has no address".to_owned(),
            });
        }
        // *Any* non-public address refuses, not "all of them": a name that answers with one
        // routable and one loopback address is the shape a rebinding attack takes.
        for address in &addresses {
            if !is_public(*address) {
                return Err(RunError::AddressNotPublic {
                    host: host.to_owned(),
                    address: *address,
                });
            }
        }
        Ok(addresses)
    }

    /// One request, with every limit and both bolts applied. The single door to the network.
    async fn issue(
        &mut self,
        url: &Url,
        method: Method,
        form: &BTreeMap<String, String>,
        depth: u32,
    ) -> Result<FetchResponse, RunError> {
        self.check_time()?;
        if depth > self.limits.max_depth {
            return Err(RunError::LimitDepth(self.limits.max_depth));
        }
        let addresses = self.guard(url).await?;
        if self.pages >= self.limits.max_pages {
            return Err(RunError::LimitPages(self.limits.max_pages));
        }
        if !self.seen.insert(cycle_key(url)) {
            return Err(RunError::Cycle {
                url: url.to_string(),
            });
        }
        self.pages += 1;
        let response = self
            .ports
            .fetcher
            .fetch(FetchRequest {
                url: url.clone(),
                addresses,
                method,
                form: form.clone(),
                max_bytes: self.limits.max_response_bytes,
                timeout: self.limits.request_timeout,
            })
            .await
            .map_err(|failure| self.transport_failure(url, failure))?;
        // The adapter was told the limit; this is the check that it kept to it.
        if response.body.len() > self.limits.max_response_bytes {
            return Err(RunError::ResponseTooLarge {
                url: url.to_string(),
                limit: self.limits.max_response_bytes,
            });
        }
        Ok(response)
    }

    /// Fetches a page, following redirects itself and checking every hop against the rule.
    /// Returns the address that finally answered together with its body.
    pub(crate) async fn fetch_page(
        &mut self,
        url: Url,
        method: Method,
        form: BTreeMap<String, String>,
    ) -> Result<(Url, FetchResponse), RunError> {
        let mut url = url;
        let mut method = method;
        let mut form = form;
        let mut depth = self.depth + 1;
        loop {
            let response = self.issue(&url, method, &form, depth).await?;
            if let Some(location) = redirect_target(&response) {
                url = url.join(location).map_err(|error| RunError::FetchFailed {
                    url: url.to_string(),
                    reason: format!("the redirect target {location:?} is not an address: {error}"),
                })?;
                // A redirect off a form submission is a GET at the target, as every browser
                // and every HTTP client does it.
                method = Method::Get;
                form.clear();
                depth += 1;
                continue;
            }
            accept_status(&url, response.status)?;
            self.depth = depth;
            self.variables.set(PAGE_URL_VARIABLE, url.to_string());
            return Ok((url, response));
        }
    }

    /// Asks one address what it redirects to, without following. The `redirect` step: the
    /// target is the value, not a page to read.
    pub(crate) async fn redirect_target_of(
        &mut self,
        step: usize,
        url: &Url,
    ) -> Result<String, RunError> {
        let depth = self.depth + 1;
        let response = self
            .issue(url, Method::Get, &BTreeMap::new(), depth)
            .await?;
        match redirect_target(&response) {
            Some(location) => {
                url.join(location)
                    .map(|target| target.to_string())
                    .map_err(|error| RunError::FetchFailed {
                        url: url.to_string(),
                        reason: format!(
                            "the redirect target {location:?} is not an address: {error}"
                        ),
                    })
            }
            None => {
                accept_status(url, response.status)?;
                Err(RunError::Structure {
                    step,
                    kind: "redirect",
                    detail: format!("{url} answered {} and no redirect", response.status),
                })
            }
        }
    }

    fn transport_failure(&self, url: &Url, failure: FetchFailure) -> RunError {
        match failure {
            FetchFailure::Unreachable(reason) => RunError::PageDead {
                url: url.to_string(),
                reason,
            },
            FetchFailure::Timeout => RunError::FetchFailed {
                url: url.to_string(),
                reason: "the request timed out".to_owned(),
            },
            FetchFailure::TooLarge => RunError::ResponseTooLarge {
                url: url.to_string(),
                limit: self.limits.max_response_bytes,
            },
            FetchFailure::Other(reason) => RunError::FetchFailed {
                url: url.to_string(),
                reason,
            },
        }
    }
}

/// What the cycle set remembers. The fragment never reaches the server, so two addresses that
/// differ only in it are one request.
fn cycle_key(url: &Url) -> String {
    let mut key = url.clone();
    key.set_fragment(None);
    key.into()
}

/// The `Location` of a redirect answer, or `None` for anything else.
fn redirect_target(response: &FetchResponse) -> Option<&str> {
    matches!(response.status, 301 | 302 | 303 | 307 | 308)
        .then(|| response.header("location"))
        .flatten()
}

/// Turns a status into the refusal that names what happened. The three that matter are told
/// apart on purpose: RD-110-09 sorts a rule by exactly this distinction.
fn accept_status(url: &Url, status: u16) -> Result<(), RunError> {
    match status {
        200..=299 => Ok(()),
        // The shapes bot protection takes. RD-110-14 measures what can be done about them.
        401 | 403 | 429 => Err(RunError::Blocked {
            url: url.to_string(),
            status,
        }),
        404 | 410 => Err(RunError::PageDead {
            url: url.to_string(),
            reason: format!("status {status}"),
        }),
        other => Err(RunError::FetchFailed {
            url: url.to_string(),
            reason: format!("status {other}"),
        }),
    }
}
