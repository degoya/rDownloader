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
//! ## Why in memory
//!
//! Failures are not persisted. Persisting them would mean a disk write per failed attempt —
//! an amplification an attacker controls — to buy a guarantee that only matters if the
//! attacker can also restart the service, at which point the lockout is not what is protecting
//! anything. A restart clears the counters, and that is a deliberate, stated trade.

use std::{
    collections::HashMap,
    net::IpAddr,
    time::{Duration, Instant},
};

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
    global_failures: u32,
    global_last_seen: Option<Instant>,
}

impl LoginThrottle {
    #[must_use]
    pub fn new(settings: ThrottleSettings) -> Self {
        Self {
            settings,
            by_address: HashMap::new(),
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

    /// Records a failed attempt and returns what the *next* one would face.
    pub fn record_failure(&mut self, address: IpAddr, now: Instant) -> Decision {
        self.expire(now);
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
        self.by_address.remove(&address);
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

    /// How many addresses are currently remembered. For diagnostics and for the tests that
    /// prove the map does not grow without bound.
    #[must_use]
    pub fn tracked_addresses(&self) -> usize {
        self.by_address.len()
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
mod tests {
    use super::*;

    fn address(value: &str) -> IpAddr {
        value.parse().expect("address")
    }

    fn throttle() -> LoginThrottle {
        LoginThrottle::new(ThrottleSettings::default())
    }

    #[test]
    fn an_ordinary_mistyped_password_costs_nothing() {
        let mut throttle = throttle();
        let now = Instant::now();
        let client = address("203.0.113.9");
        for _ in 0..2 {
            throttle.record_failure(client, now);
        }
        assert_eq!(
            throttle.check(client, now),
            Decision::Proceed {
                delay: Duration::ZERO
            }
        );
    }

    #[test]
    fn sustained_guessing_from_one_address_is_locked_out() {
        let mut throttle = throttle();
        let now = Instant::now();
        let client = address("203.0.113.9");
        for _ in 0..6 {
            throttle.record_failure(client, now);
        }
        assert!(matches!(
            throttle.check(client, now),
            Decision::Locked { .. }
        ));
    }

    /// The whole point: the attacker's lockout must not be the owner's.
    #[test]
    fn one_address_being_locked_out_does_not_lock_out_another() {
        let mut throttle = throttle();
        let now = Instant::now();
        let attacker = address("203.0.113.9");
        let owner = address("192.168.1.20");
        for _ in 0..20 {
            throttle.record_failure(attacker, now);
        }
        assert!(matches!(
            throttle.check(attacker, now),
            Decision::Locked { .. }
        ));
        assert!(matches!(
            throttle.check(owner, now),
            Decision::Proceed { .. }
        ));
    }

    /// The distributed case, and the reason the global counter exists at all.
    #[test]
    fn many_addresses_failing_once_each_still_slow_the_endpoint_down() {
        let mut throttle = throttle();
        let now = Instant::now();
        for index in 0..40_u8 {
            throttle.record_failure(address(&format!("203.0.113.{index}")), now);
        }
        let Decision::Proceed { delay } = throttle.check(address("198.51.100.1"), now) else {
            panic!("the global counter must not lock anyone out");
        };
        assert!(delay > Duration::ZERO);
    }

    /// …and it must never be able to close the door, however long the attack runs.
    #[test]
    fn the_global_counter_can_never_refuse_an_attempt() {
        let mut throttle = throttle();
        let now = Instant::now();
        for index in 0..2_000_u32 {
            throttle.record_failure(
                address(&format!(
                    "10.{}.{}.{}",
                    index / 65536,
                    (index / 256) % 256,
                    index % 256
                )),
                now,
            );
        }
        match throttle.check(address("198.51.100.1"), now) {
            Decision::Proceed { delay } => {
                assert!(
                    delay <= ThrottleSettings::default().max_global_delay,
                    "the owner would wait {delay:?}"
                );
            }
            Decision::Locked { .. } => panic!("an attacker locked the owner out"),
        }
    }

    #[test]
    fn the_lockout_lengthens_with_each_further_failure() {
        let mut throttle = throttle();
        let now = Instant::now();
        let client = address("203.0.113.9");
        for _ in 0..6 {
            throttle.record_failure(client, now);
        }
        let Decision::Locked { retry_after: first } = throttle.check(client, now) else {
            panic!("expected a lockout");
        };
        throttle.record_failure(client, now);
        let Decision::Locked {
            retry_after: second,
        } = throttle.check(client, now)
        else {
            panic!("expected a lockout");
        };
        assert!(second > first, "{second:?} should exceed {first:?}");
    }

    #[test]
    fn the_lockout_is_capped() {
        let mut throttle = throttle();
        let now = Instant::now();
        let client = address("203.0.113.9");
        for _ in 0..200 {
            throttle.record_failure(client, now);
        }
        let Decision::Locked { retry_after } = throttle.check(client, now) else {
            panic!("expected a lockout");
        };
        assert!(retry_after <= ThrottleSettings::default().max_lockout);
    }

    #[test]
    fn a_lockout_ends_when_its_time_is_up() {
        let mut throttle = throttle();
        let now = Instant::now();
        let client = address("203.0.113.9");
        for _ in 0..6 {
            throttle.record_failure(client, now);
        }
        let later = now + Duration::from_secs(60 * 60);
        assert!(matches!(
            throttle.check(client, later),
            Decision::Proceed { .. }
        ));
    }

    /// Getting the password right clears what an attacker built up.
    #[test]
    fn a_success_resets_both_counters() {
        let mut throttle = throttle();
        let now = Instant::now();
        let owner = address("192.168.1.20");
        for index in 0..40_u8 {
            throttle.record_failure(address(&format!("203.0.113.{index}")), now);
        }
        throttle.record_failure(owner, now);
        throttle.record_success(owner, now);
        assert_eq!(
            throttle.check(owner, now),
            Decision::Proceed {
                delay: Duration::ZERO
            }
        );
    }

    /// An attacker cycling through addresses must not be able to grow the map without bound.
    #[test]
    fn quiet_addresses_are_forgotten() {
        let mut throttle = throttle();
        let now = Instant::now();
        for index in 0..50_u8 {
            throttle.record_failure(address(&format!("203.0.113.{index}")), now);
        }
        assert_eq!(throttle.tracked_addresses(), 50);
        let much_later = now + Duration::from_secs(6 * 60 * 60);
        throttle.check(address("198.51.100.1"), much_later);
        assert_eq!(throttle.tracked_addresses(), 0);
    }

    /// …but an address still serving a lockout is kept, or the lockout would evaporate.
    ///
    /// Uses settings where a lockout outlasts the idle window, because with the defaults it
    /// never can — and a test that cannot reach the branch it names is not testing it.
    #[test]
    fn an_address_serving_a_lockout_is_not_forgotten() {
        let settings = ThrottleSettings {
            failures_before_lockout: 1,
            base_lockout: Duration::from_secs(60 * 60),
            max_lockout: Duration::from_secs(24 * 60 * 60),
            window: Duration::from_secs(60),
            ..ThrottleSettings::default()
        };
        let mut throttle = LoginThrottle::new(settings);
        let now = Instant::now();
        let client = address("203.0.113.9");
        for _ in 0..3 {
            throttle.record_failure(client, now);
        }

        // Well past the idle window, and still inside the lockout.
        let later = now + Duration::from_secs(10 * 60);
        assert!(matches!(
            throttle.check(client, later),
            Decision::Locked { .. }
        ));
        assert_eq!(
            throttle.tracked_addresses(),
            1,
            "the address was forgotten while it was still serving its lockout"
        );
    }
}
