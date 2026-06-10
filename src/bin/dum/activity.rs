//! Sparse per-node live-activity state: EWMA rates, decay, sparkline rings.

use std::collections::HashMap;
use std::time::Instant;

use crate::tree::NodeId;

pub const RING_BUCKETS: usize = 30;
/// Glow decay time constant (seconds).
pub const TAU_SECS: f64 = 2.5;
const ALPHA: f64 = 0.3;
/// Entries idle longer than this are evicted (ring fully aged out).
const EVICT_AFTER_SECS: f64 = 35.0;

struct Activity {
    rate: f64, // EWMA of signed bytes/sec
    last_update: Instant,
    ring: [i64; RING_BUCKETS],
    last_bucket: u64,
}

pub struct ActivityMap {
    map: HashMap<NodeId, Activity>,
    start: Instant,
}

impl ActivityMap {
    pub fn new(start: Instant) -> Self {
        ActivityMap { map: HashMap::new(), start }
    }

    pub fn record(&mut self, id: NodeId, delta: i64, now: Instant) {
        let bucket = now.duration_since(self.start).as_secs();
        let a = self.map.entry(id).or_insert(Activity {
            rate: 0.0,
            last_update: now,
            ring: [0; RING_BUCKETS],
            last_bucket: bucket,
        });
        let dt = now.duration_since(a.last_update).as_secs_f64().max(0.05);
        a.rate = ALPHA * (delta as f64 / dt) + (1.0 - ALPHA) * a.rate;
        a.last_update = now;
        // Zero any skipped buckets, then add into the current one.
        let from = a.last_bucket + 1;
        for b in from..=bucket {
            a.ring[(b % RING_BUCKETS as u64) as usize] = 0;
        }
        a.last_bucket = bucket;
        a.ring[(bucket % RING_BUCKETS as u64) as usize] += delta;
    }

    /// Signed bytes/sec with exponential age decay; 0.0 when unknown.
    pub fn glow(&self, id: NodeId, now: Instant) -> f64 {
        self.map
            .get(&id)
            .map(|a| {
                let age = now.duration_since(a.last_update).as_secs_f64();
                a.rate * (-age / TAU_SECS).exp()
            })
            .unwrap_or(0.0)
    }

    /// Ring contents ordered oldest..newest (last element = bucket of last activity).
    pub fn sparkline(&self, id: NodeId) -> Option<[i64; RING_BUCKETS]> {
        self.map.get(&id).map(|a| {
            let mut out = [0i64; RING_BUCKETS];
            for (i, slot) in out.iter_mut().enumerate() {
                let b = a.last_bucket + 1 + i as u64; // oldest first
                *slot = a.ring[(b % RING_BUCKETS as u64) as usize];
            }
            out
        })
    }

    pub fn evict(&mut self, now: Instant) {
        self.map
            .retain(|_, a| now.duration_since(a.last_update).as_secs_f64() < EVICT_AFTER_SECS);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn record_positive_delta_gives_positive_glow() {
        let t0 = Instant::now();
        let mut m = ActivityMap::new(t0);
        m.record(1, 1_000_000, t0 + Duration::from_secs(1));
        assert!(m.glow(1, t0 + Duration::from_secs(1)) > 0.0);
    }

    #[test]
    fn negative_delta_gives_negative_glow() {
        let t0 = Instant::now();
        let mut m = ActivityMap::new(t0);
        m.record(1, -500_000, t0 + Duration::from_secs(1));
        assert!(m.glow(1, t0 + Duration::from_secs(1)) < 0.0);
    }

    #[test]
    fn glow_decays_exponentially_after_activity_stops() {
        let t0 = Instant::now();
        let mut m = ActivityMap::new(t0);
        let t1 = t0 + Duration::from_secs(1);
        m.record(1, 1_000_000, t1);
        let fresh = m.glow(1, t1);
        let later = m.glow(1, t1 + Duration::from_secs(5));
        assert!(later < fresh * 0.2); // 5s = 2*tau -> e^-2 ~= 0.135
        assert!(later > 0.0);
    }

    #[test]
    fn unknown_node_has_zero_glow_and_no_sparkline() {
        let t0 = Instant::now();
        let m = ActivityMap::new(t0);
        assert_eq!(m.glow(42, t0), 0.0);
        assert!(m.sparkline(42).is_none());
    }

    #[test]
    fn ring_buckets_deltas_by_second_oldest_first() {
        let t0 = Instant::now();
        let mut m = ActivityMap::new(t0);
        m.record(1, 100, t0 + Duration::from_secs(1));
        m.record(1, 200, t0 + Duration::from_secs(1));
        m.record(1, 50, t0 + Duration::from_secs(3));
        let ring = m.sparkline(1).unwrap();
        // newest bucket (second 3) is last; second 1 holds 300; gap second 2 is 0
        assert_eq!(ring[RING_BUCKETS - 1], 50);
        assert_eq!(ring[RING_BUCKETS - 3], 300);
        assert_eq!(ring[RING_BUCKETS - 2], 0);
    }

    #[test]
    fn evict_drops_stale_entries() {
        let t0 = Instant::now();
        let mut m = ActivityMap::new(t0);
        m.record(1, 100, t0 + Duration::from_secs(1));
        m.evict(t0 + Duration::from_secs(2));
        assert!(m.sparkline(1).is_some());
        m.evict(t0 + Duration::from_secs(60));
        assert!(m.sparkline(1).is_none());
    }
}
