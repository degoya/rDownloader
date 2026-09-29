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
//! `rd-siterules` keeps its own copy of the ranges (`exec/guard.rs`): it is a leaf crate that
//! depends on neither `rd-core` nor this one, and it only ever needs "public or not".

use std::{
    error::Error as StdError,
    fmt,
    future::Future,
    io,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    pin::Pin,
    sync::Arc,
};

use url::{Host, Url};

/// How far from the internet an address is. Ordered from the most to the least reachable, so
/// the stricter of two judgements is their maximum.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum AddressScope {
    /// Routable on the internet.
    Public,
    /// The person's own network: RFC 1918, carrier-grade NAT, IPv6 unique-local and the
    /// deprecated site-local block.
    Private,
    /// Never the home of a remote file: loopback, unspecified, link-local (the cloud metadata
    /// endpoint among it), multicast, broadcast and the reserved and special-purpose blocks.
    Local,
}

/// Which scope an address belongs to. An IPv4 address wearing an IPv6 costume is judged as the
/// IPv4 address it carries *as well as* by the IPv6 rules, and the stricter answer wins.
#[must_use]
pub fn address_scope(address: IpAddr) -> AddressScope {
    match address {
        IpAddr::V4(v4) => scope_v4(v4),
        IpAddr::V6(v6) => {
            let own = scope_v6(v6);
            embedded_v4(v6).map_or(own, |v4| own.max(scope_v4(v4)))
        }
    }
}

fn scope_v4(address: Ipv4Addr) -> AddressScope {
    let [a, b, c, _] = address.octets();
    let local = address.is_loopback()
        || address.is_link_local()
        || address.is_unspecified()
        || address.is_multicast()
        || address.is_broadcast()
        // 0.0.0.0/8, "this network".
        || a == 0
        // 192.0.0.0/24, IETF protocol assignments.
        || (a == 192 && b == 0 && c == 0)
        // 198.18.0.0/15, benchmarking.
        || (a == 198 && (b == 18 || b == 19))
        // 240.0.0.0/4, reserved.
        || a >= 240;
    if local {
        AddressScope::Local
    } else if address.is_private()
        // 100.64.0.0/10, carrier-grade NAT: the provider's network, not the internet.
        || (a == 100 && (64..128).contains(&b))
    {
        AddressScope::Private
    } else {
        AddressScope::Public
    }
}

fn scope_v6(address: Ipv6Addr) -> AddressScope {
    let segments = address.segments();
    let local = address.is_loopback()
        || address.is_unspecified()
        || address.is_multicast()
        // fe80::/10, link local.
        || (segments[0] & 0xffc0) == 0xfe80
        // 100::/64, discard-only (RFC 6666).
        || (segments[0] == 0x0100 && segments[1..4].iter().all(|segment| *segment == 0))
        // 64:ff9b:1::/48, NAT64 with a network-specific prefix (RFC 8215): the embedded
        // address cannot be read out without knowing the prefix length, so the block goes.
        || (segments[0] == 0x0064 && segments[1] == 0xff9b && segments[2] == 0x0001)
        // 2001::/32, Teredo: the packet goes to a relay, not to the address inside.
        || (segments[0] == 0x2001 && segments[1] == 0x0000)
        // 2001:2::/48, benchmarking (RFC 5180).
        || (segments[0] == 0x2001 && segments[1] == 0x0002 && segments[2] == 0)
        // 2001:10::/28 and 2001:20::/28, ORCHID and ORCHIDv2.
        || (segments[0] == 0x2001 && matches!(segments[1] & 0xfff0, 0x0010 | 0x0020))
        // 2001:db8::/32 and 3fff::/20, documentation.
        || (segments[0] == 0x2001 && segments[1] == 0x0db8)
        || (segments[0] & 0xfff0) == 0x3ff0;
    if local {
        AddressScope::Local
    } else if (segments[0] & 0xfe00) == 0xfc00 || (segments[0] & 0xffc0) == 0xfec0 {
        // fc00::/7 unique local, fec0::/10 the site-local block it replaced.
        AddressScope::Private
    } else {
        AddressScope::Public
    }
}

/// The IPv4 address an IPv6 address carries where the address itself says so: IPv4-mapped,
/// 6to4, the NAT64 well-known prefix, an ISATAP interface identifier and IPv4-compatible.
/// Without this, `::ffff:127.0.0.1` reads as an ordinary IPv6 address.
fn embedded_v4(address: Ipv6Addr) -> Option<Ipv4Addr> {
    let segments = address.segments();
    let octets = address.octets();
    let last_32 = || Ipv4Addr::new(octets[12], octets[13], octets[14], octets[15]);
    if let Some(mapped) = address.to_ipv4_mapped() {
        return Some(mapped);
    }
    // 2002:aabb:ccdd::/48, 6to4: the IPv4 address sits in bits 16 to 47.
    if segments[0] == 0x2002 {
        return Some(Ipv4Addr::new(octets[2], octets[3], octets[4], octets[5]));
    }
    // 64:ff9b::/96, the NAT64 well-known prefix: the last 32 bits.
    if segments[0] == 0x0064
        && segments[1] == 0xff9b
        && segments[2..6].iter().all(|segment| *segment == 0)
    {
        return Some(last_32());
    }
    // ...:0000:5efe:a.b.c.d and ...:0200:5efe:a.b.c.d, ISATAP under any prefix.
    if matches!(segments[4], 0x0000 | 0x0200) && segments[5] == 0x5efe {
        return Some(last_32());
    }
    // ::a.b.c.d, IPv4-compatible; `::` and `::1` are not, and the IPv6 rules refuse them.
    if segments[..6].iter().all(|segment| *segment == 0) && !(segments[6] == 0 && segments[7] <= 1)
    {
        return Some(last_32());
    }
    None
}

/// The literal address a URL's host *is*, when it is one rather than a name.
#[must_use]
pub fn literal_address(url: &Url) -> Option<IpAddr> {
    match url.host()? {
        Host::Ipv4(address) => Some(IpAddr::V4(address)),
        Host::Ipv6(address) => Some(IpAddr::V6(address)),
        Host::Domain(_) => None,
    }
}

/// Which addresses a request made on a stranger's word may reach.
///
/// Public addresses always; the person's own network only when they asked for it (a mirror
/// set they pasted themselves, a tracker — a self-hosted one on the LAN is a real setup); this
/// machine never: every [`AddressScope::Local`] address, and the addresses the service itself
/// listens on.
#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub struct AddressPolicy {
    local_network: bool,
    /// The addresses this service answers on, beyond loopback.
    this_machine: Vec<IpAddr>,
}

impl AddressPolicy {
    /// Public addresses, plus the person's own network when `local_network`.
    #[must_use]
    pub fn new(local_network: bool) -> Self {
        Self {
            local_network,
            this_machine: Vec::new(),
        }
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
            AddressScope::Local => false,
        }
    }

    /// Why a redirect to `target` may not be followed, or `None` when it may. Only HTTP(S),
    /// and a literal address has to be one the policy permits; a name is left to the resolver,
    /// which a guarded client always has.
    #[must_use]
    pub fn hop_refusal(&self, target: &Url) -> Option<AddressRefused> {
        let host = target.host_str().unwrap_or_default().to_owned();
        if !matches!(target.scheme(), "http" | "https") || host.is_empty() {
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
