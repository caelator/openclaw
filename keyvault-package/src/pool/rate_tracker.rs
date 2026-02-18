//! In-memory rate tracker — pre-flight RPM/RPD accounting per key per model.
//!
//! Tracks request timestamps to calculate current RPM and RPD consumption
//! for each (key_id, model_id) pair. The swarm scheduler checks this BEFORE
//! assigning a request to avoid wasting a call that will 429.
//!
//! Thread-safe via `RwLock<HashMap>` — no external dependencies needed.

use std::collections::{HashMap, VecDeque};
use std::sync::RwLock;
use std::time::{Duration, Instant, SystemTime};

use serde::Serialize;

/// Composite key for rate tracking: (key_id, model_id).
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct RateKey {
    key_id: String,
    model_id: String,
}

/// Sliding-window counters for a single (key, model) pair.
struct RateWindow {
    /// Timestamps of requests in the last 60 seconds (for RPM)
    minute_window: VecDeque<Instant>,
    /// Count of requests today (resets at midnight)
    daily_count: u32,
    /// The date (day number) for the daily counter
    daily_reset_day: u32,
}

impl RateWindow {
    fn new() -> Self {
        Self {
            minute_window: VecDeque::new(),
            daily_count: 0,
            daily_reset_day: current_day(),
        }
    }

    /// Record a new request.
    fn record(&mut self) {
        let now = Instant::now();
        self.minute_window.push_back(now);
        self.prune_minute_window(now);

        let today = current_day();
        if today != self.daily_reset_day {
            self.daily_count = 0;
            self.daily_reset_day = today;
        }
        self.daily_count += 1;
    }

    /// Current requests-per-minute (sliding window).
    fn current_rpm(&mut self) -> u32 {
        self.prune_minute_window(Instant::now());
        self.minute_window.len() as u32
    }

    /// Current requests-per-day.
    fn current_rpd(&mut self) -> u32 {
        let today = current_day();
        if today != self.daily_reset_day {
            self.daily_count = 0;
            self.daily_reset_day = today;
        }
        self.daily_count
    }

    /// Remove timestamps older than 60 seconds.
    fn prune_minute_window(&mut self, now: Instant) {
        let cutoff = now - Duration::from_secs(60);
        while let Some(&front) = self.minute_window.front() {
            if front < cutoff {
                self.minute_window.pop_front();
            } else {
                break;
            }
        }
    }
}

/// Get current day number for daily reset tracking.
fn current_day() -> u32 {
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    (now.as_secs() / 86400) as u32
}

// ── Public API ──────────────────────────────────────────────────────

/// Thread-safe rate tracker for all key/model pairs.
pub struct RateTracker {
    windows: RwLock<HashMap<RateKey, RateWindow>>,
}

impl RateTracker {
    pub fn new() -> Self {
        Self {
            windows: RwLock::new(HashMap::new()),
        }
    }

    /// Record that a request was sent with this key + model.
    pub fn record_request(&self, key_id: &str, model_id: &str) {
        let key = RateKey {
            key_id: key_id.into(),
            model_id: model_id.into(),
        };
        let mut map = self.windows.write().unwrap();
        map.entry(key)
            .or_insert_with(RateWindow::new)
            .record();
    }

    /// Check if a request can be made without exceeding limits.
    /// Returns `(can_proceed, current_rpm, current_rpd)`.
    pub fn check_capacity(
        &self,
        key_id: &str,
        model_id: &str,
        max_rpm: u16,
        max_rpd: u32,
    ) -> (bool, u32, u32) {
        let key = RateKey {
            key_id: key_id.into(),
            model_id: model_id.into(),
        };

        let mut map = self.windows.write().unwrap();
        match map.get_mut(&key) {
            Some(window) => {
                let rpm = window.current_rpm();
                let rpd = window.current_rpd();
                let can_proceed = rpm < max_rpm as u32 && rpd < max_rpd;
                (can_proceed, rpm, rpd)
            }
            None => (true, 0, 0),
        }
    }

    /// Find the least-loaded key for a given model.
    /// Returns the key_id with the lowest current RPM that's under limits.
    pub fn least_loaded_key<'a>(
        &self,
        keys: &'a [String],
        model_id: &str,
        max_rpm: u16,
        max_rpd: u32,
    ) -> Option<&'a String> {
        let mut best: Option<(&String, u32)> = None;

        for key_id in keys {
            let (can_proceed, rpm, _rpd) = self.check_capacity(key_id, model_id, max_rpm, max_rpd);
            if !can_proceed {
                continue;
            }
            match best {
                None => best = Some((key_id, rpm)),
                Some((_, best_rpm)) if rpm < best_rpm => best = Some((key_id, rpm)),
                _ => {}
            }
        }

        best.map(|(k, _)| k)
    }

    /// Get a snapshot of all tracked rates for the dashboard.
    pub fn snapshot(&self) -> Vec<RateSnapshot> {
        let mut map = self.windows.write().unwrap();
        map.iter_mut()
            .map(|(key, window)| RateSnapshot {
                key_id: key.key_id.clone(),
                model_id: key.model_id.clone(),
                current_rpm: window.current_rpm(),
                current_rpd: window.current_rpd(),
            })
            .collect()
    }
}

/// A point-in-time snapshot of rate usage for a key/model pair.
#[derive(Debug, Clone, Serialize)]
pub struct RateSnapshot {
    pub key_id: String,
    pub model_id: String,
    pub current_rpm: u32,
    pub current_rpd: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_record_and_check() {
        let tracker = RateTracker::new();
        let (ok, rpm, rpd) = tracker.check_capacity("key1", "model1", 10, 100);
        assert!(ok);
        assert_eq!(rpm, 0);
        assert_eq!(rpd, 0);

        tracker.record_request("key1", "model1");
        let (ok, rpm, rpd) = tracker.check_capacity("key1", "model1", 10, 100);
        assert!(ok);
        assert_eq!(rpm, 1);
        assert_eq!(rpd, 1);
    }

    #[test]
    fn test_rpm_limit_respected() {
        let tracker = RateTracker::new();
        for _ in 0..5 {
            tracker.record_request("key1", "model1");
        }
        let (ok, rpm, _) = tracker.check_capacity("key1", "model1", 5, 1000);
        assert!(!ok, "Should be at RPM limit");
        assert_eq!(rpm, 5);
    }

    #[test]
    fn test_least_loaded_key() {
        let tracker = RateTracker::new();
        let keys = vec!["k1".into(), "k2".into(), "k3".into()];

        for _ in 0..3 { tracker.record_request("k1", "m"); }
        tracker.record_request("k2", "m");

        let best = tracker.least_loaded_key(&keys, "m", 10, 1000);
        assert_eq!(best, Some(&"k3".to_string()), "k3 is least loaded");
    }

    #[test]
    fn test_least_loaded_skips_full_keys() {
        let tracker = RateTracker::new();
        let keys = vec!["k1".into(), "k2".into()];

        for _ in 0..5 { tracker.record_request("k1", "m"); }
        tracker.record_request("k2", "m");

        let best = tracker.least_loaded_key(&keys, "m", 5, 1000);
        assert_eq!(best, Some(&"k2".to_string()), "k1 is full, should pick k2");
    }

    #[test]
    fn test_snapshot() {
        let tracker = RateTracker::new();
        tracker.record_request("k1", "m1");
        tracker.record_request("k1", "m2");
        tracker.record_request("k2", "m1");

        let snap = tracker.snapshot();
        assert_eq!(snap.len(), 3);
    }
}
