//! Bounded failure windows, shared by all connections and forwarding addresses.
use std::{
    collections::{HashMap, VecDeque},
    net::IpAddr,
    time::{Duration, Instant},
};

const WINDOW: Duration = Duration::from_secs(60);
const CAPACITY: usize = 4096;
const LIMITED: &str = "Too many login attempts. Wait a minute.";

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Key {
    Ip(IpAddr),
    Account(String),
}

pub(crate) struct Reservation(Vec<(Key, Instant)>);

#[derive(Default)]
pub(crate) struct LoginLimits {
    counts: HashMap<Key, (Instant, u32)>,
    // Windows never refresh: expiration is ordered and each key is queued once.
    expires: VecDeque<(Instant, Key)>,
}
impl LoginLimits {
    pub(crate) fn admit(
        &mut self,
        ip: IpAddr,
        user: &str,
        now: Instant,
    ) -> Result<Reservation, &'static str> {
        while self
            .expires
            .front()
            .is_some_and(|(at, _)| now.duration_since(*at) >= WINDOW)
        {
            if let Some((_, key)) = self.expires.pop_front() {
                self.counts.remove(&key);
            }
        }
        let keys = [
            (Key::Ip(ip.to_canonical()), 10),
            (Key::Account(user.to_owned()), 20),
        ];
        let missing = keys
            .iter()
            .filter(|(key, _)| !self.counts.contains_key(key))
            .count();
        if self.counts.len() + missing > CAPACITY
            || keys
                .iter()
                .any(|(key, limit)| self.counts.get(key).is_some_and(|(_, n)| n >= limit))
        {
            return Err(LIMITED);
        }
        let mut reservation = Vec::with_capacity(2);
        for (key, _) in keys {
            let count = self.counts.entry(key.clone()).or_insert_with(|| {
                self.expires.push_back((now, key.clone()));
                (now, 0)
            });
            count.1 += 1;
            reservation.push((key, count.0));
        }
        Ok(Reservation(reservation))
    }

    pub(crate) fn finish(&mut self, reservation: Reservation, failed: bool) {
        if !failed {
            for (key, started) in reservation.0 {
                if let Some((at, count)) = self.counts.get_mut(&key)
                    && *at == started
                {
                    *count = count.saturating_sub(1);
                }
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    #[test]
    fn failures_are_limited_by_ip_and_account_and_expire() {
        let mut limits = LoginLimits::default();
        let now = Instant::now();
        let ip = "198.51.100.1".parse().unwrap();
        for n in 0..10 {
            limits.admit(ip, &format!("user{n}"), now).unwrap();
        }
        assert!(limits.admit(ip, "another", now).is_err());
        for n in 1..=20 {
            limits
                .admit(
                    IpAddr::V4(std::net::Ipv4Addr::new(203, 0, 113, n)),
                    "target",
                    now,
                )
                .unwrap();
        }
        assert!(
            limits
                .admit("203.0.113.99".parse().unwrap(), "target", now)
                .is_err()
        );
        limits.admit(ip, "target", now + WINDOW).unwrap();
        assert_eq!(limits.counts.len(), 2);
        assert_eq!(limits.expires.len(), 2);
    }
    #[test]
    fn successful_and_unavailable_attempts_refund_only_their_reservation() {
        let mut limits = LoginLimits::default();
        let now = Instant::now();
        let ip = "198.51.100.1".parse().unwrap();
        for _ in 0..100 {
            let reservation = limits.admit(ip, "alice", now).unwrap();
            limits.finish(reservation, false);
        }
        assert_eq!(limits.expires.len(), 2);
        for _ in 0..9 {
            limits.admit(ip, "alice", now).unwrap();
        }
        let bob = limits.admit(ip, "bob", now).unwrap();
        assert!(limits.admit(ip, "alice", now).is_err());
        limits.finish(bob, false);
        limits.admit(ip, "alice", now).unwrap();
        assert!(limits.admit(ip, "alice", now).is_err());
    }
    #[test]
    fn late_completions_cannot_refund_a_new_window() {
        let mut limits = LoginLimits::default();
        let now = Instant::now();
        let ip = "198.51.100.1".parse().unwrap();
        let old = limits.admit(ip, "alice", now).unwrap();
        for _ in 0..10 {
            limits.admit(ip, "alice", now + WINDOW).unwrap();
        }
        limits.finish(old, false);
        assert!(limits.admit(ip, "alice", now + WINDOW).is_err());
    }
    #[test]
    fn saturation_is_bounded_and_does_not_evict_active_limits() {
        let mut limits = LoginLimits::default();
        let now = Instant::now();
        for n in 0..CAPACITY / 2 {
            let ip = IpAddr::V6(std::net::Ipv6Addr::from(u128::try_from(n + 1).unwrap()));
            limits.admit(ip, &format!("user{n}"), now).unwrap();
        }
        assert!(
            limits
                .admit("198.51.100.1".parse().unwrap(), "new", now)
                .is_err()
        );
        assert_eq!(limits.counts.len(), CAPACITY);
        assert_eq!(limits.expires.len(), CAPACITY);
        limits
            .admit("198.51.100.1".parse().unwrap(), "new", now + WINDOW)
            .unwrap();
        assert_eq!(limits.counts.len(), 2);
    }
}
