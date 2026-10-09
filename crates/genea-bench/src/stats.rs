//! Percentiles over samples, as the spikes computed them (nearest rank:
//! the sample at index `round((n − 1) · q)` of the sorted samples).

use serde_json::{Value, json};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Summary {
    pub n: usize,
    pub p50: f64,
    pub p95: f64,
    pub p99: f64,
    pub max: f64,
}

impl Summary {
    /// `None` without samples.
    pub fn of(samples: &[f64]) -> Option<Summary> {
        if samples.is_empty() {
            return None;
        }
        let mut sorted = samples.to_vec();
        sorted.sort_by(f64::total_cmp);
        let pick = |q: f64| sorted[((sorted.len() - 1) as f64 * q).round() as usize];
        Some(Summary { n: sorted.len(), p50: pick(0.5), p95: pick(0.95), p99: pick(0.99), max: pick(1.0) })
    }

    pub fn to_json(&self) -> Value {
        json!({ "n": self.n, "p50": round(self.p50), "p95": round(self.p95), "p99": round(self.p99), "max": round(self.max) })
    }
}

/// A summary as JSON, or `null` without samples.
pub fn summary_json(samples: &[f64]) -> Value {
    Summary::of(samples).map_or(Value::Null, |s| s.to_json())
}

/// Rounds to two decimals for reports.
pub fn round(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}
