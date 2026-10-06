//! Where a request made on a stranger's word may go (RD-150-03).
//!
//! A Metalink document names mirrors and a `.torrent` names trackers, and the service then
//! requests those addresses on the person's behalf. Written by someone else, such an address is
//! a server-side request forgery primitive: `127.0.0.1:8710` is this service's own API,
//! `169.254.169.254` a cloud's metadata endpoint, `192.168.0.1` the person's router.
//! [`AddressPolicy`] says which addresses such a request may reach, and it is checked twice:
//!
//! - **before the request**, by [`check_target`], against the literal address or against every
//!   address the name resolves to — *any* refused address refuses the name, since a name that
//!   answers with one public and one loopback address is the shape a rebinding attack takes;
//! - **when the connection is made**, by [`GuardedResolver`], the DNS resolver a guarded client
//!   is built with. The client connects only to addresses this resolver handed it, so a name
//!   that answered the first lookup with a public address and the second with a loopback one
//!   is refused at the second.
//!
//! A literal address never reaches a resolver. It is judged by itself: before the request by
//! [`check_target`], and on every redirect hop by [`AddressPolicy::hop_refusal`], which the
//! client pool bakes into a guarded client's redirect policy.
//!
//! Which range an address falls in is `rd_core::address_scope`, the one classification this
//! guard and the site-rule guard (`rd-siterules`, `exec/guard.rs`) share. Each used to keep its
//! own copy, and the copies drifted apart.

use std::{
    error::Error as StdError,
    fmt,
    future::Future,
    io,
    net::{IpAddr, SocketAddr},
    pin::Pin,
    sync::Arc,
};

use url::{Host, Url};

// The classification itself is `rd_core::address_scope`, shared with `rd-siterules` so the two
// guards cannot drift apart again (security audit 2026-09-30, R1).
pub use rd_core::{AddressScope, address_scope, literal_address};

/// Which addresses a request made on a stranger's word may reach.
///
/// Public addresses always; the person's own network only when they asked for it (a mirror
/// set they pasted themselves, a tracker — a self-hosted one on the LAN is a real setup); this
/// machine never: every [`AddressScope::Local`] address, and the addresses the service itself
/// listens on.
#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub struct AddressPolicy {
    local_network: bool,
    /// Whether loopback addresses are permitted after all (RA-HOST-01, owner 2026-10-04): for
    /// a plugin request to an address the person entered, on a port none of the service's own.
    loopback: bool,
    /// The addresses this service answers on, beyond loopback.
    this_machine: Vec<IpAddr>,
    /// Ports no redirect may lead to, whatever the host: the service's own listeners. A hop
    /// may change the port, and a name is judged by the resolver, which never sees one.
    refused_ports: Vec<u16>,
}

impl AddressPolicy {
    /// Public addresses, plus the person's own network when `local_network`.
    #[must_use]
    pub fn new(local_network: bool) -> Self {
        Self {
            local_network,
            loopback: false,
            this_machine: Vec::new(),
            refused_ports: Vec::new(),
        }
    }

    /// Also permits loopback — `127.0.0.0/8`, `::1` and an IPv4 loopback inside an IPv6
    /// address — though no other address [`AddressScope::Local`] covers: link-local, with a
    /// cloud's metadata endpoint, stays refused. For a request to an address the person
    /// entered, which may name a server on this machine.
    #[must_use]
    pub fn with_loopback(mut self) -> Self {
        self.loopback = true;
        self
    }

    /// Refuses every redirect to one of `ports`, on any host.
    #[must_use]
    pub fn refusing_redirects_to(mut self, ports: &[u16]) -> Self {
        let mut ports = ports.to_vec();
        ports.sort_unstable();
        ports.dedup();
        self.refused_ports = ports;
        self
    }

    /// Also refuses the address the service listens on — or, listening on every interface,
    /// every address this machine has, since each of them reaches the service's own API.
    #[must_use]
    pub fn listening_on(mut self, listen: Option<SocketAddr>) -> Self {
        let Some(listen) = listen else {
            return self;
        };
        let mut own = if listen.ip().is_unspecified() {
            interface_addresses()
        } else {
            vec![listen.ip().to_canonical()]
        };
        own.sort_unstable();
        own.dedup();
        self.this_machine = own;
        self
    }

    /// Whether a request may connect to `address`.
    #[must_use]
    pub fn permits(&self, address: IpAddr) -> bool {
        let address = address.to_canonical();
        if self.this_machine.contains(&address) {
            return false;
        }
        match address_scope(address) {
            AddressScope::Public => true,
            AddressScope::Private => self.local_network,
            AddressScope::Local => self.loopback && address.is_loopback(),
        }
    }

    /// Why a redirect to `target` may not be followed, or `None` when it may. Only HTTP(S),
    /// and a literal address has to be one the policy permits; a name is left to the resolver,
    /// which a guarded client always has.
    #[must_use]
    pub fn hop_refusal(&self, target: &Url) -> Option<AddressRefused> {
        let host = target.host_str().unwrap_or_default().to_owned();
        if !matches!(target.scheme(), "http" | "https")
            || host.is_empty()
            || target
                .port_or_known_default()
                .is_some_and(|port| self.refused_ports.contains(&port))
        {
            return Some(AddressRefused {
                host,
                address: None,
            });
        }
        let address = literal_address(target)?;
        (!self.permits(address)).then_some(AddressRefused {
            host,
            address: Some(address),
        })
    }

    /// The addresses a name answered with, or the refusal of the first one the policy does
    /// not permit. An empty answer is a name without an address, not a refusal.
    fn screen(&self, host: &str, addresses: Vec<IpAddr>) -> Result<Vec<IpAddr>, TargetRefusal> {
        if addresses.is_empty() {
            return Err(TargetRefusal::Unresolved(io::Error::new(
                io::ErrorKind::NotFound,
                format!("{host} has no address"),
            )));
        }
        if let Some(refused) = addresses.iter().find(|address| !self.permits(**address)) {
            return Err(TargetRefusal::Refused(AddressRefused {
                host: host.to_owned(),
                address: Some(*refused),
            }));
        }
        Ok(addresses)
    }
}

/// Every address of this machine's interfaces; empty when they cannot be listed.
fn interface_addresses() -> Vec<IpAddr> {
    use network_interface::NetworkInterfaceConfig as _;

    network_interface::NetworkInterface::show()
        .map(|found| {
            found
                .iter()
                .flat_map(|interface| interface.addr.iter().map(|address| address.ip()))
                .map(|address| address.to_canonical())
                .collect()
        })
        .unwrap_or_default()
}

/// A target the policy refuses. Travels as the source of a transport error, so the engine can
/// tell this refusal from an ordinary network failure (see [`is_refusal`]).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AddressRefused {
    pub host: String,
    /// The refused address; `None` when the scheme or the missing host was the reason.
    pub address: Option<IpAddr>,
}

impl fmt::Display for AddressRefused {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.address {
            Some(address) => write!(
                formatter,
                "{} points at {address}, which a remote document may not reach",
                self.host
            ),
            None => write!(
                formatter,
                "{} is not an address a remote document may point at",
                self.host
            ),
        }
    }
}

impl StdError for AddressRefused {}

/// Why a target was not requested.
#[derive(Debug)]
pub enum TargetRefusal {
    /// The address is, or the name resolves to, one the policy refuses.
    Refused(AddressRefused),
    /// The name has no address. Nothing was refused, and nothing can be fetched either.
    Unresolved(io::Error),
}

impl fmt::Display for TargetRefusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Refused(refused) => fmt::Display::fmt(refused, formatter),
            Self::Unresolved(error) => write!(formatter, "the name could not be resolved: {error}"),
        }
    }
}

impl StdError for TargetRefusal {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match self {
            Self::Refused(refused) => Some(refused),
            Self::Unresolved(error) => Some(error),
        }
    }
}

/// Whether an error, or anything it was caused by, is this guard's refusal — from the
/// resolver of a guarded client or from its redirect policy.
#[must_use]
pub fn is_refusal(error: &(dyn StdError + 'static)) -> bool {
    let mut current = Some(error);
    while let Some(error) = current {
        if error.is::<AddressRefused>() {
            return true;
        }
        current = error.source();
    }
    false
}

/// A lookup's answer.
pub type LookupFuture<'a> = Pin<Box<dyn Future<Output = io::Result<Vec<IpAddr>>> + Send + 'a>>;

/// Resolves a host name to addresses. A trait so a test can make a name answer with anything.
pub trait HostLookup: Send + Sync {
    fn lookup<'a>(&'a self, host: &'a str) -> LookupFuture<'a>;
}

/// The operating system's resolver, as every other request of the service uses it.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemLookup;

impl HostLookup for SystemLookup {
    fn lookup<'a>(&'a self, host: &'a str) -> LookupFuture<'a> {
        Box::pin(async move {
            Ok(tokio::net::lookup_host((host, 0))
                .await?
                .map(|address| address.ip())
                .collect())
        })
    }
}

/// Checks a target before it is requested: the literal address, or every address its name
/// resolves to. Returns the checked addresses; a caller that connects by itself (a UDP
/// tracker) connects to exactly those instead of resolving the name a second time.
pub async fn check_target(
    policy: &AddressPolicy,
    lookup: &dyn HostLookup,
    target: &Url,
) -> Result<Vec<IpAddr>, TargetRefusal> {
    let name = match target.host() {
        None => {
            return Err(TargetRefusal::Refused(AddressRefused {
                host: String::new(),
                address: None,
            }));
        }
        Some(Host::Ipv4(address)) => {
            return policy.screen(&address.to_string(), vec![address.into()]);
        }
        Some(Host::Ipv6(address)) => {
            return policy.screen(&address.to_string(), vec![address.into()]);
        }
        Some(Host::Domain(name)) => name,
    };
    let addresses = lookup
        .lookup(name)
        .await
        .map_err(TargetRefusal::Unresolved)?;
    policy.screen(name, addresses)
}

/// The socket addresses a connection to `host:port` may use, for a transport that opens its
/// own sockets — FTP and SFTP (RD-150-03).
///
/// The same rule as [`check_target`], and the connection-time half of it at once: the caller
/// connects to exactly the addresses returned and never resolves the name again, so a name
/// that answers with a public address now cannot answer with a loopback one a moment later.
/// A refusal travels inside an [`io::Error`] of kind `PermissionDenied`, the error a
/// transport's connect already returns; [`refusal_in`] finds it there. `host` may be a
/// bracketed IPv6 literal, as a URL spells one.
pub async fn connect_addresses(
    policy: &AddressPolicy,
    lookup: &dyn HostLookup,
    host: &str,
    port: u16,
) -> io::Result<Vec<SocketAddr>> {
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    let screened = match bare.parse::<IpAddr>() {
        Ok(address) => policy.screen(bare, vec![address]),
        Err(_) => policy.screen(bare, lookup.lookup(bare).await?),
    };
    match screened {
        Ok(addresses) => Ok(addresses
            .into_iter()
            .map(|address| SocketAddr::new(address, port))
            .collect()),
        Err(TargetRefusal::Refused(refused)) => {
            Err(io::Error::new(io::ErrorKind::PermissionDenied, refused))
        }
        Err(TargetRefusal::Unresolved(error)) => Err(error),
    }
}

/// The addresses a transport that opens its own sockets connects to: under a guard, exactly
/// those [`connect_addresses`] admits (a refusal is an I/O error [`refusal_in`] recognises);
/// without one, every address `host` resolves to, as a plain connect would try them. FTP and
/// SFTP each carried this (RD-1110-04).
pub async fn socket_addresses(
    guard: Option<&AddressPolicy>,
    host: &str,
    port: u16,
) -> io::Result<Vec<SocketAddr>> {
    match guard {
        Some(policy) => connect_addresses(policy, &SystemLookup, host, port).await,
        None => Ok(tokio::net::lookup_host(format!("{host}:{port}"))
            .await?
            .collect()),
    }
}

/// The guard's refusal an I/O error carries, when [`connect_addresses`] refused the target.
#[must_use]
pub fn refusal_in(error: &io::Error) -> Option<&AddressRefused> {
    error.get_ref()?.downcast_ref::<AddressRefused>()
}

/// The DNS resolver of a guarded client: resolves like the system does and refuses a name
/// with any address the policy does not permit, at the moment the connection is made.
#[derive(Clone)]
pub struct GuardedResolver {
    policy: AddressPolicy,
    lookup: Arc<dyn HostLookup>,
}

impl GuardedResolver {
    /// A resolver over the system's lookup.
    #[must_use]
    pub fn system(policy: AddressPolicy) -> Self {
        Self::with_lookup(policy, Arc::new(SystemLookup))
    }

    /// A resolver over any lookup; tests use it to make a name answer with a chosen address.
    #[must_use]
    pub fn with_lookup(policy: AddressPolicy, lookup: Arc<dyn HostLookup>) -> Self {
        Self { policy, lookup }
    }
}

impl reqwest::dns::Resolve for GuardedResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let policy = self.policy.clone();
        let lookup = Arc::clone(&self.lookup);
        let host = name.as_str().to_owned();
        Box::pin(async move {
            let answered = lookup.lookup(&host).await?;
            let permitted = match policy.screen(&host, answered) {
                Ok(permitted) => permitted,
                // The refusal itself, not the wrapper, so `is_refusal` finds it in the chain.
                Err(TargetRefusal::Refused(refused)) => return Err(refused.into()),
                Err(TargetRefusal::Unresolved(error)) => return Err(error.into()),
            };
            // Port 0: the connector sets the URL's port on every address it is handed.
            let addresses: reqwest::dns::Addrs = Box::new(
                permitted
                    .into_iter()
                    .map(|address| SocketAddr::new(address, 0)),
            );
            Ok(addresses)
        })
    }
}

#[cfg(test)]
#[path = "address_guard_tests.rs"]
mod tests;
