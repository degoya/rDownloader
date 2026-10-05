//! The limiter's curve, one behaviour per test, without a clock: every instant is passed in.

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

/// Audit 2026-10-05, S4: a client with a /64 rotated through its addresses and never built up
/// the per-address lockout.
#[test]
fn an_ipv6_client_is_counted_by_its_slash_64() {
    let mut throttle = throttle();
    let now = Instant::now();
    for index in 0..6_u16 {
        throttle.record_failure(address(&format!("2001:db8:1:2::{index:x}")), now);
    }
    assert!(matches!(
        throttle.check(address("2001:db8:1:2:ffff:ffff:ffff:ffff"), now),
        Decision::Locked { .. }
    ));
    assert!(matches!(
        throttle.check(address("2001:db8:1:3::1"), now),
        Decision::Proceed { .. }
    ));
    assert_eq!(throttle.tracked_addresses(), 1);
}

#[test]
fn an_ipv4_mapped_address_is_its_ipv4_address() {
    let mut throttle = throttle();
    let now = Instant::now();
    for _ in 0..6 {
        throttle.record_failure(address("::ffff:203.0.113.9"), now);
    }
    assert!(matches!(
        throttle.check(address("203.0.113.9"), now),
        Decision::Locked { .. }
    ));
    assert!(matches!(
        throttle.check(address("203.0.113.10"), now),
        Decision::Proceed { .. }
    ));
}

/// Audit 2026-10-05, S4: parallel attempts all passed the door before the first failure was
/// counted. An address gets no more attempts at once than sequential ones before its lockout.
#[test]
fn parallel_attempts_are_counted_before_their_verdict() {
    let mut throttle = throttle();
    let now = Instant::now();
    let client = address("203.0.113.9");
    let allowed = ThrottleSettings::default().failures_before_lockout + 1;
    for _ in 0..allowed {
        assert!(matches!(
            throttle.admit(client, now),
            Decision::Proceed { .. }
        ));
    }
    let Decision::Locked { retry_after } = throttle.admit(client, now) else {
        panic!("an attempt beyond the failures left was let in");
    };
    assert!(retry_after <= Duration::from_secs(1));
    // Another address is not touched by this one's attempts.
    assert!(matches!(
        throttle.admit(address("192.168.1.20"), now),
        Decision::Proceed { .. }
    ));
    // One verdict in, one slot free.
    throttle.release(client);
    assert!(matches!(
        throttle.admit(client, now),
        Decision::Proceed { .. }
    ));
}

#[test]
fn failures_shrink_how_many_attempts_may_run_at_once() {
    let mut throttle = throttle();
    let now = Instant::now();
    let client = address("203.0.113.9");
    for _ in 0..ThrottleSettings::default().failures_before_lockout {
        throttle.record_failure(client, now);
    }
    assert!(matches!(
        throttle.admit(client, now),
        Decision::Proceed { .. }
    ));
    assert!(matches!(
        throttle.admit(client, now),
        Decision::Locked { .. }
    ));
}

/// A lockout that ran out still leaves one attempt, however many failures came before it.
#[test]
fn an_expired_lockout_admits_one_attempt_at_a_time() {
    let mut throttle = throttle();
    let now = Instant::now();
    let client = address("203.0.113.9");
    for _ in 0..8 {
        throttle.record_failure(client, now);
    }
    let later = now + ThrottleSettings::default().max_lockout + Duration::from_secs(1);
    assert!(matches!(
        throttle.admit(client, later),
        Decision::Proceed { .. }
    ));
    assert!(matches!(
        throttle.admit(client, later),
        Decision::Locked { .. }
    ));
    throttle.release(client);
    throttle.release(client);
    assert!(matches!(
        throttle.admit(client, later),
        Decision::Proceed { .. }
    ));
}
