//! How many HTTP requests one invocation may hand to the host, and how much of the time spent
//! waiting on them is credited back to its execution deadline.
//!
//! The waiting time of a request is credited so that a slow server is not mistaken for a hung
//! plugin. Without a bound on that credit, the request count and the response bytes were the
//! only things ending an invocation that kept asking; both are now bounded per invocation,
//! generously enough that no ordinary crawl or resolution comes near either.

use std::time::Duration;

use rd_core::{Failure, FailureKind};

use super::PluginStoreState;

/// Most HTTP requests one invocation may make through the host.
///
/// A deep crawl is the heaviest bundled caller and stays far below this; the cap exists so an
/// invocation has an end that does not depend on the far side answering slowly.
pub(crate) const MAX_HOST_HTTP_REQUESTS: u32 = 2_000;

/// Most waiting time on HTTP requests one invocation may have credited to its deadline.
pub(crate) const MAX_CREDITED_HTTP_TIME: Duration = Duration::from_secs(30 * 60);

/// What one invocation has spent of its request allowance so far.
#[derive(Debug, Default)]
pub(crate) struct HostRequestBudget {
    requests: u32,
    credited: Duration,
}

impl PluginStoreState {
    /// Counts one more host HTTP request, or refuses it once the invocation has made as many
    /// as it may or has been credited all the waiting time it may.
    pub(crate) fn claim_http_request(&mut self) -> Result<(), Failure> {
        let budget = &mut self.host_requests;
        if budget.requests >= MAX_HOST_HTTP_REQUESTS || budget.credited >= MAX_CREDITED_HTTP_TIME {
            return Err(Failure::coded(
                FailureKind::Permanent,
                "plugin.host_requests_exhausted",
                format!(
                    "A plugin call may make at most {MAX_HOST_HTTP_REQUESTS} host requests \
                     and wait on them for at most {} seconds",
                    MAX_CREDITED_HTTP_TIME.as_secs()
                ),
            )
            .with_param("limit", MAX_HOST_HTTP_REQUESTS));
        }
        budget.requests += 1;
        Ok(())
    }

    /// Credits the time spent waiting on one HTTP request, up to what the invocation has left
    /// of [`MAX_CREDITED_HTTP_TIME`].
    pub(crate) fn credit_http_time(&mut self, elapsed: Duration) {
        let remaining = MAX_CREDITED_HTTP_TIME.saturating_sub(self.host_requests.credited);
        let credit = elapsed.min(remaining);
        self.host_requests.credited += credit;
        self.credit_host_time(credit);
    }

    /// Host HTTP requests made so far, for tests.
    #[cfg(test)]
    pub(crate) fn host_http_requests(&self) -> u32 {
        self.host_requests.requests
    }
}
