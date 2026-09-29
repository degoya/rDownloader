//! The component: the five calls of the resolver world.
//!
//! Replace the provider-specific parts — the account check and the request `resolve` makes —
//! with your provider's; [`crate::claims`] decides which links are yours.
#![allow(unsafe_code)] // Generated canonical-ABI exports contain the only unsafe code here.

wit_bindgen::generate!({
    path: "wit",
    world: "resolver-plugin",
});

use exports::rdownloader::plugin::resolver::Guest;
use rdownloader::plugin::{
    host,
    http::{self, RequestHeader},
    types::{
        AccountStatus, CheckRequest, Failure, FailureKind, LabelPart, LinkCheckResult, LinkStatus,
        ResolveRequest, ResolvedDownload, ResolvedHeader,
    },
};

/// The credential reference this plugin may expand, as declared in `manifest.toml`.
const SECRET: &str = "{{PLUGIN_SLUG}}_password";

struct Component;

impl Guest for Component {
    fn match_url(url: String) -> bool {
        crate::claims(&url)
    }

    fn check_account(account_id: String) -> Result<AccountStatus, Failure> {
        if !host::secret_available(&account_id, SECRET) {
            return Err(failure(
                FailureKind::AuthRequired,
                "{{PLUGIN_SLUG}}.no_credential",
                "No stored password for this account",
            ));
        }
        // Replace with a call to your provider's account endpoint.
        //
        // `premium` is a finding, not a hope: answer `true` only where this call actually read
        // the subscription. A scaffold has read nothing, so it answers `false` and says so in
        // the label. Shipping `true` here taught every plugin built from this template to claim
        // a subscription nobody checked, which is what RD-109-34 and RD-109-38 had to undo in
        // three of them.
        //
        // The label is a list of translatable parts, each a code with parameters and an
        // English fallback text -- the same shape as `Failure { code, params, message }`.
        // `plugin.account.*` codes are translated by the core in every language; a code of
        // your own (`{{PLUGIN_SLUG}}.*`) needs an entry in your `locales/*.json`. Free text
        // without a code is refused by the host.
        Ok(AccountStatus {
            valid: true,
            premium: false,
            label: vec![LabelPart {
                code: "plugin.account.premium_unchecked".to_owned(),
                params: Vec::new(),
                message: "the subscription was not checked".to_owned(),
            }],
            traffic_left: None,
        })
    }

    fn resolve(request: ResolveRequest) -> Result<ResolvedDownload, Failure> {
        // `{{secret:…}}` is expanded by the host: the value never enters the component.
        let response = http::http_request(
            "GET",
            &request.url,
            &[],
            &[RequestHeader {
                name: "authorization".to_owned(),
                value_template: format!("Bearer {{{{secret:{SECRET}}}}}"),
            }],
            &[],
        )?;
        if response.status != 200 {
            return Err(failure(
                FailureKind::Permanent,
                "{{PLUGIN_SLUG}}.rejected",
                "The provider refused the request",
            ));
        }
        Ok(ResolvedDownload {
            url: response.final_url,
            file_name: None,
            size: None,
            headers: Vec::<ResolvedHeader>::new(),
            checksum_algorithm: None,
            checksum_value: None,
            client: request.client,
        })
    }

    fn check(request: CheckRequest) -> Result<Vec<LinkCheckResult>, Failure> {
        Ok(request
            .urls
            .into_iter()
            .map(|url| LinkCheckResult {
                url,
                status: LinkStatus::Unknown,
                file_name: None,
                size: None,
            })
            .collect())
    }

    fn hosters(_account_id: String) -> Result<Vec<String>, Failure> {
        Ok(crate::HOSTS.iter().map(|host| (*host).to_owned()).collect())
    }
}

fn failure(category: FailureKind, code: &str, message: &str) -> Failure {
    Failure {
        category,
        message: message.to_owned(),
        code: Some(code.to_owned()),
        params: Vec::new(),
    }
}

export!(Component);
