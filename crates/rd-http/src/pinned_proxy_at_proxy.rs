//! A guarded client behind a proxy that resolves names itself ([`super::NameResolution::AtProxy`],
//! RD-1420-01, audit L-07).
//!
//! Such a proxy is handed the name, so no resolver of this client ever sees a target, and a
//! redirect hop to a name went out unjudged: [`crate::AddressPolicy::hop_refusal`] judges only
//! literals and leaves a name to a resolver this client does not have. reqwest's redirect policy
//! cannot judge a name itself, since it runs synchronously and a lookup does not; so every
//! connection is judged instead, the first request's and each hop's alike, before a byte of it
//! goes to the proxy.
//!
//! The first proxy of such a client is `connect_target::recorder`, which notes each connect
//! call's target and declines, so the profile's proxy behind it is the one used, exactly as
//! before. [`JudgeLayer`] runs around the connect call, takes the noted target and judges it as
//! [`super::check_before_request`] does before the connection is made: a name that resolves here
//! to an address the rule refuses is refused, and one this machine cannot resolve is left to the
//! proxy, which is what the profile says it is for. On a port refused on this machine alone an
//! answer naming this machine refuses it too. A connection whose target was never noted is
//! refused rather than made unjudged.

use std::{
    error::Error as StdError,
    future::Future,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};

use crate::{AddressPolicy, AddressRefused, HostLookup, TargetRefusal, connect_target};

type BoxError = Box<dyn StdError + Send + Sync>;

/// The proxy that notes each connect call's target and declines it, for the next proxy.
pub(super) fn recorder() -> reqwest::Proxy {
    connect_target::recorder()
}

/// The connector layer that judges each connection's target before it is made.
#[derive(Clone)]
pub(super) struct JudgeLayer {
    pub(super) policy: AddressPolicy,
    pub(super) lookup: Arc<dyn HostLookup>,
}

impl<S> tower::Layer<S> for JudgeLayer {
    type Service = JudgeService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        JudgeService {
            inner,
            policy: self.policy.clone(),
            lookup: Arc::clone(&self.lookup),
        }
    }
}

#[derive(Clone)]
pub(super) struct JudgeService<S> {
    inner: S,
    policy: AddressPolicy,
    lookup: Arc<dyn HostLookup>,
}

impl<S, R> tower::Service<R> for JudgeService<S>
where
    S: tower::Service<R, Error = BoxError>,
    S::Future: Send + 'static,
    S::Response: Send + 'static,
{
    type Response = S::Response;
    type Error = BoxError;
    type Future = Pin<Box<dyn Future<Output = Result<S::Response, BoxError>> + Send>>;

    fn poll_ready(&mut self, context: &mut Context<'_>) -> Poll<Result<(), BoxError>> {
        self.inner.poll_ready(context)
    }

    fn call(&mut self, request: R) -> Self::Future {
        let (connecting, target) = connect_target::noting(|| self.inner.call(request));
        let policy = self.policy.clone();
        let lookup = Arc::clone(&self.lookup);
        Box::pin(async move {
            let Some(target) = target else {
                return Err(Box::new(AddressRefused {
                    host: String::new(),
                    address: None,
                }) as BoxError);
            };
            judge(&policy, lookup.as_ref(), &target)
                .await
                .map_err(|refused| Box::new(refused) as BoxError)?;
            connecting.await
        })
    }
}

/// [`super::check_before_request`] for a name the proxy resolves, and on a port refused on this
/// machine alone (RD-1420-01) an answer naming this machine refused as well.
async fn judge(
    policy: &AddressPolicy,
    lookup: &dyn HostLookup,
    target: &url::Url,
) -> Result<(), AddressRefused> {
    let local = policy.refuses_this_machine_on(target.port_or_known_default());
    match crate::check_target(policy, lookup, target).await {
        Ok(addresses) => match addresses
            .into_iter()
            .find(|address| local && policy.is_this_machine(*address))
        {
            Some(address) => Err(AddressRefused {
                host: target.host_str().unwrap_or_default().to_owned(),
                address: Some(address),
            }),
            None => Ok(()),
        },
        Err(TargetRefusal::Unresolved(_)) => Ok(()),
        Err(TargetRefusal::Refused(refused)) => Err(refused),
    }
}
