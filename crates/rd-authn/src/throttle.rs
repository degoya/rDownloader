//! Login rate limiting that an attacker cannot turn into a denial of service.
//!
//! ## The trap
//!
//! This installation has one account. Counting failures against *the account* and locking it
//! is therefore the obvious design and the wrong one: anyone who can reach the login form can
//! then lock the owner out of their own service by failing on purpose, indefinitely. The
//! protection becomes the attack.
//!
//! Counting against the *address* alone fails differently. Behind a reverse proxy or a
//! carrier NAT every request shares one address, so one attacker locks out everybody; and an
//! attacker with many addresses is barely slowed.
//!
//! ## The shape
//!
//! Two counters that do different jobs.
//!
//! * **Per address — a hard lockout.** Brute force comes from few addresses, so this is the
//!   one that actually stops it. A locked-out address is refused outright, and someone else's
//!   address is unaffected: the owner logging in from their laptop is not touched by an
//!   attacker hammering from elsewhere.
//! * **Globally — a delay that never becomes a refusal.** This covers the distributed case,
//!   where every attempt arrives from a fresh address and the per-address counter never
//!   builds. It slows the whole login endpoint down and is deliberately incapable of closing
//!   it: the worst an attacker can inflict on the owner is a wait, capped at a few seconds.
//!
//! A success resets both. The owner getting their password right clears whatever an attacker
//! built up.
//!
//! ## What counts as one address
//!
//! An IPv4 address is one address. An IPv6 address is counted by its /64: that is the block a
//! single subscriber, host or container is handed, and counting each of its 2^64 addresses
//! apart let one client rotate through them and never build up a lockout (audit 2026-10-05,
//! S4). An IPv4-mapped IPv6 address is its IPv4 address.
//!
//! ## Attempts in flight
//!
//! The counters only learn of a failure once the password check is over. Checked at the door
//! alone, any number of parallel attempts from one address all passed before the first of them
//! was counted. An admitted attempt therefore counts against its address as though it had
//! already failed until its verdict is in ([`LoginThrottle::admit`], [`LoginThrottle::release`]):
//! an address gets no more attempts at once than it has failures left before its next lockout,
//! and at least one, so a lockout that ran out still leaves the next try.
//!
//! ## Why in memory
//!
//! Failures are not persisted. Persisting them would mean a disk write per failed attempt —
//! an amplification an attacker controls — to buy a guarantee that only matters if the
//! attacker can also restart the service, at which point the lockout is not what is protecting
//! anything. A restart clears the counters, and that is a deliberate, stated trade.

use std::{
    collections::HashMap,
    net::{IpAddr, Ipv6Addr},
    time::{Duration, Instant},
};

use crate::cidr::unmap;

/// How long an address whose every remaining attempt is in flight is asked to wait. Short:
/// the attempts it waits for end in a second or so.
const IN_FLIGHT_RETRY: Duration = Duration::from_secs(1);

/// How aggressive the limiter is.
#[derive(Clone, Copy, Debug)]
pub struct ThrottleSettings {
    /// Failures from one address before it is locked out at all.
    ///
    /// Generous on purpose: a person mistyping a password twice is ordinary, and a limiter
    /// that punishes ordinary use gets switched off.
    pub failures_before_lockout: u32,
    /// How long the first lockout lasts. Each further failure doubles it.
    pub base_lockout: Duration,
    /// The ceiling on that doubling.
    pub max_lockout: Duration,
    /// Failures across all addresses before every attempt is delayed.
    pub failures_before_global_delay: u32,
    /// How long each further global failure adds.
    pub global_delay_step: Duration,
    /// The ceiling on the global delay. Must stay small: this is the part an attacker can
    /// impose on the owner, so it is a nuisance by design and never a lockout.
    pub max_global_delay: Duration,
    /// How long a quiet address is remembered before its failures are forgotten.
    pub window: Duration,
}

impl Default for ThrottleSettings {
    fn default() -> Self {
        Self {
            failures_before_lockout: 5,
            base_lockout: Duration::from_secs(15),
            max_lockout: Duration::from_secs(15 * 60),
            failures_before_global_delay: 20,
            global_delay_step: Duration::from_millis(250),
            max_global_delay: Duration::from_secs(2),
            window: Duration::from_secs(60 * 60),
        }
    }
}

/// What to do with an attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Decision {
    /// Proceed, after waiting this long. Zero when nothing is going on.
    Proceed { delay: Duration },
    /// Refuse without checking the password, for this long.
    Locked { retry_after: Duration },
}

#[derive(Clone, Copy, Debug)]
struct Attempts {
    failures: u32,
    locked_until: Option<Instant>,
    last_seen: Instant,
}

/// Per-address and global login failure counters.
#[derive(Debug)]
pub struct LoginThrottle {
    settings: ThrottleSettings,
    by_address: HashMap<IpAddr, Attempts>,
    /// Admitted attempts whose verdict is not in yet, by the same key as `by_address`.
    in_flight: HashMap<IpAddr, u32>,
    global_failures: u32,
    global_last_seen: Option<Instant>,
}

impl LoginThrottle {
    #[must_use]
    pub fn new(settings: ThrottleSettings) -> Self {
        Self {
            settings,
            by_address: HashMap::new(),
            in_flight: HashMap::new(),
            global_failures: 0,
            global_last_seen: None,
        }
    }

    /// What should happen to an attempt from `address` at `now`.
    ///
    /// Takes the instant rather than reading the clock so the curve is testable without
    /// sleeping through it.
    pub fn check(&mut self, address: IpAddr, now: Instant) -> Decision {
        self.expire(now);
        let address = key(address);
        if let Some(attempts) = self.by_address.get(&address)
            && let Some(until) = attempts.locked_until
            && until > now
        {
            return Decision::Locked {
                retry_after: until.saturating_duration_since(now),
            };
        }
        Decision::Proceed {
            delay: self.global_delay(),
        }
    }

    /// [`Self::check`], and on `Proceed` the attempt is admitted: it counts against its address
    /// until [`Self::release`] hands it back, whatever its verdict.
    ///
    /// Refused with a short `retry_after` when the address already has as many attempts in
    /// flight as it has failures left before its next lockout. The `delay` of an admission is
    /// the global one, which the caller has normally paid already, between its `check` and
    /// this call.
    pub fn admit(&mut self, address: IpAddr, now: Instant) -> Decision {
        let decision = self.check(address, now);
        if matches!(decision, Decision::Locked { .. }) {
            return decision;
        }
        let address = key(address);
        let failures = self
            .by_address
            .get(&address)
            .map_or(0, |attempts| attempts.failures);
        let allowed = self
            .settings
            .failures_before_lockout
            .saturating_add(1)
            .saturating_sub(failures)
            .max(1);
        let pending = self.in_flight.entry(address).or_insert(0);
        if *pending >= allowed {
            return Decision::Locked {
                retry_after: IN_FLIGHT_RETRY,
            };
        }
        *pending += 1;
        decision
    }

    /// Hands back an attempt [`Self::admit`] let in. Its verdict, if it had one, was recorded
    /// on its own.
    pub fn release(&mut self, address: IpAddr) {
        let address = key(address);
        if let Some(pending) = self.in_flight.get_mut(&address) {
            *pending = pending.saturating_sub(1);
            if *pending == 0 {
                self.in_flight.remove(&address);
            }
        }
    }

    /// Records a failed attempt and returns what the *next* one would face.
    pub fn record_failure(&mut self, address: IpAddr, now: Instant) -> Decision {
        self.expire(now);
        let address = key(address);
        let settings = self.settings;
        let attempts = self.by_address.entry(address).or_insert(Attempts {
            failures: 0,
            locked_until: None,
            last_seen: now,
        });
        attempts.failures = attempts.failures.saturating_add(1);
        attempts.last_seen = now;
        if attempts.failures > settings.failures_before_lockout {
            let over = attempts.failures - settings.failures_before_lockout;
            attempts.locked_until = Some(now + lockout_for(over, &settings));
        }
        self.global_failures = self.global_failures.saturating_add(1);
        self.global_last_seen = Some(now);
        self.check(address, now)
    }

    /// Clears everything this address and the installation had built up.
    pub fn record_success(&mut self, address: IpAddr, now: Instant) {
        self.expire(now);
        self.by_address.remove(&key(address));
        self.global_failures = 0;
        self.global_last_seen = Some(now);
    }

    /// The delay every attempt currently pays.
    fn global_delay(&self) -> Duration {
        let settings = &self.settings;
        if self.global_failures <= settings.failures_before_global_delay {
            return Duration::ZERO;
        }
        let over = self.global_failures - settings.failures_before_global_delay;
        settings
            .global_delay_step
            .saturating_mul(over)
            .min(settings.max_global_delay)
    }

    /// Forgets addresses and global counters that have gone quiet.
    ///
    /// Also what keeps the map from growing without bound: an attacker cycling through
    /// addresses would otherwise leave an entry behind for each one.
    fn expire(&mut self, now: Instant) {
        let window = self.settings.window;
        self.by_address.retain(|_, attempts| {
            let locked = attempts.locked_until.is_some_and(|until| until > now);
            locked || now.saturating_duration_since(attempts.last_seen) < window
        });
        if let Some(last) = self.global_last_seen
            && now.saturating_duration_since(last) >= window
        {
            self.global_failures = 0;
            self.global_last_seen = None;
        }
    }

    /// How many addresses are currently remembered, for the tests that prove the map does not
    /// grow without bound.
    #[cfg(test)]
    #[must_use]
    pub fn tracked_addresses(&self) -> usize {
        self.by_address.len()
    }
}

/// The key `address` is counted under: an IPv6 address by its /64, anything else as itself.
#[must_use]
pub fn key(address: IpAddr) -> IpAddr {
    match unmap(address) {
        IpAddr::V6(v6) => IpAddr::V6(Ipv6Addr::from(u128::from(v6) & !u128::from(u64::MAX))),
        v4 => v4,
    }
}

/// Doubling backoff, capped.
fn lockout_for(over_limit: u32, settings: &ThrottleSettings) -> Duration {
    let shift = over_limit.saturating_sub(1).min(20);
    settings
        .base_lockout
        .saturating_mul(1_u32 << shift)
        .min(settings.max_lockout)
}

#[cfg(test)]
#[path = "throttle_tests.rs"]
mod tests;
