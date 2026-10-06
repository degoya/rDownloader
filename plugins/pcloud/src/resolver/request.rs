//! The calls themselves: one installation at a time, and the one correction between the two.

use pcloud_common::{address::Region, api as pcloud_api};
use plugin_common::failure::coded;
use plugin_common::{Failure, FailureKind, HttpRequest, PluginHost};

use super::SECRET;
use crate::{api, messages};

/// One refusal pCloud made: its own number, and the wait it asked for if it asked for one.
struct Refusal {
    result: u64,
    retry_after: Option<u64>,
}

/// Why one call did not produce an answer.
enum Rejected {
    /// The host refused, the transport failed, or the document was not pCloud's. Final.
    Fatal(Failure),
    /// pCloud answered, and the answer was no. May be worth asking the other installation.
    Refused(Refusal),
}

/// One call at one installation, with no correction.
async fn once<H: PluginHost>(
    host: &H,
    region: Region,
    method: &str,
    query: &[(&'static str, String)],
    authenticated: bool,
) -> Result<Vec<u8>, Rejected> {
    let mut request = HttpRequest::get(format!("{}/{method}", region.api()));
    for (name, value) in query {
        request = request.with_query(name, value.clone());
    }
    if authenticated {
        request = request.with_header("Authorization", format!("Bearer {{{{secret:{SECRET}}}}}"));
    }
    let response = host.http(request).await.map_err(Rejected::Fatal)?;
    if !(200..300).contains(&response.status) {
        // pCloud answers 200 to its own refusals, so a status that is not 2xx never came from
        // pCloud's application: it is a gateway or the network.
        return Err(Rejected::Fatal(Failure::coded(
            FailureKind::Transient(pcloud_api::retry_after(&response.headers)),
            messages::UNAVAILABLE.0,
            messages::UNAVAILABLE.1,
        )));
    }
    let Some(result) = pcloud_api::result_of(&response.body) else {
        return Err(Rejected::Fatal(coded(
            FailureKind::Permanent,
            messages::INVALID_RESPONSE,
        )));
    };
    if result == pcloud_api::OK {
        return Ok(response.body);
    }
    Err(Rejected::Refused(Refusal {
        result,
        retry_after: pcloud_api::retry_after(&response.headers),
    }))
}

/// One call at an installation that is already settled. No correction, because there is
/// nothing left to correct.
pub(super) async fn fixed<H: PluginHost>(
    host: &H,
    region: Region,
    method: &str,
    query: &[(&'static str, String)],
    authenticated: bool,
) -> Result<Vec<u8>, Failure> {
    once(host, region, method, query, authenticated)
        .await
        .map_err(|rejected| match rejected {
            Rejected::Fatal(failure) => failure,
            Rejected::Refused(refusal) => fail(&refusal),
        })
}

/// One call, corrected once if pCloud's own answer says the installation was wrong.
///
/// The correction is deliberately narrow (see the comment of the `resolver` module): only a refused credential
/// and a refused link code, only once, and only the region that actually answered is returned
/// — so the caller pins it and the rest of the invocation costs nothing extra.
pub(super) async fn call<H: PluginHost>(
    host: &H,
    start: Region,
    method: &str,
    query: &[(&'static str, String)],
    authenticated: bool,
) -> Result<(Vec<u8>, Region), Failure> {
    let mut carried: Option<Failure> = None;
    for (attempt, region) in start.both_from().into_iter().enumerate() {
        match once(host, region, method, query, authenticated).await {
            Ok(body) => return Ok((body, region)),
            Err(Rejected::Fatal(failure)) => return Err(failure),
            Err(Rejected::Refused(refusal)) => {
                let worth_the_other_region = attempt == 0
                    && pcloud_api::Category::of(refusal.result).may_be_the_other_region();
                if !worth_the_other_region {
                    return Err(fail(&refusal));
                }
                host.log(
                    "debug",
                    "pcloud refused this at the first data centre; asking the other one",
                );
                carried = Some(fail(&refusal));
            }
        }
    }
    Err(carried.unwrap_or_else(|| coded(FailureKind::Permanent, messages::INVALID_RESPONSE)))
}

/// Turns one pCloud refusal into a failure, carrying its number and nothing else.
fn fail(refusal: &Refusal) -> Failure {
    let ((code, message), kind) = api::classify(refusal.result, refusal.retry_after);
    // pCloud's own decimal number. It is an integer, so unlike an `error` sentence there is
    // nothing in it that could ever have been a token, a file name or a path.
    Failure::coded(kind, code, message).with_param("result", refusal.result.to_string())
}
