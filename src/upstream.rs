use crate::config::Config;
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct UpstreamPool {
    pub addr: String,
    pub weight: u32,
}

pub struct LoadBalancer {
    upstreams: Vec<UpstreamPool>,
    counter: AtomicUsize,
}

impl LoadBalancer {
    pub fn new(upstreams: Vec<UpstreamPool>) -> Self {
        // Expand upstreams into an interleaved weighted round-robin schedule.
        // Weights are reduced by their GCD first so weight=100/100 produces
        // [a, b] rather than 200 entries.
        let gcd_all = upstreams.iter().map(|u| u.weight.max(1)).fold(0, gcd);
        let reduced: Vec<UpstreamPool> = upstreams
            .into_iter()
            .map(|u| UpstreamPool {
                weight: u.weight.max(1) / gcd_all.max(1),
                ..u
            })
            .collect();

        let max_weight = reduced.iter().map(|u| u.weight).max().unwrap_or(1);
        let mut expanded = Vec::new();
        for i in 0..max_weight {
            for u in reduced.iter() {
                if i < u.weight {
                    expanded.push(u.clone());
                }
            }
        }

        LoadBalancer {
            upstreams: expanded,
            counter: AtomicUsize::new(0),
        }
    }

    pub fn next(&self) -> Option<&UpstreamPool> {
        if self.upstreams.is_empty() {
            return None;
        }
        let idx = self.counter.fetch_add(1, Ordering::Relaxed) % self.upstreams.len();
        self.upstreams.get(idx)
    }
}

fn gcd(a: u32, b: u32) -> u32 {
    if b == 0 {
        a
    } else {
        gcd(b, a % b)
    }
}

/// Build one LoadBalancer per route from a config. Called at startup and
/// again on every successful reload so upstream changes take effect.
pub fn build_lb_map(config: &Config) -> HashMap<String, Arc<LoadBalancer>> {
    config
        .route
        .iter()
        .map(|route| {
            let upstreams = route
                .upstream
                .iter()
                .map(|u| UpstreamPool {
                    addr: u.addr.clone(),
                    weight: u.weight.unwrap_or(100),
                })
                .collect();
            (route.name.clone(), Arc::new(LoadBalancer::new(upstreams)))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pool(addr: &str, weight: u32) -> UpstreamPool {
        UpstreamPool {
            addr: addr.to_string(),
            weight,
        }
    }

    #[test]
    fn empty_returns_none() {
        let lb = LoadBalancer::new(vec![]);
        assert!(lb.next().is_none());
    }

    #[test]
    fn round_robin_alternates_equal_weights() {
        let lb = LoadBalancer::new(vec![pool("a", 50), pool("b", 50)]);
        let picks: Vec<&str> = (0..4).map(|_| lb.next().unwrap().addr.as_str()).collect();
        assert_eq!(picks, vec!["a", "b", "a", "b"]);
    }

    #[test]
    fn weights_reduce_by_gcd() {
        // 100/100 should expand to a schedule of length 2, not 200
        let lb = LoadBalancer::new(vec![pool("a", 100), pool("b", 100)]);
        assert_eq!(lb.upstreams.len(), 2);
    }

    #[test]
    fn respects_weight_ratio() {
        let lb = LoadBalancer::new(vec![pool("a", 75), pool("b", 25)]);
        let mut counts = std::collections::HashMap::new();
        for _ in 0..100 {
            *counts.entry(lb.next().unwrap().addr.clone()).or_insert(0) += 1;
        }
        assert_eq!(counts["a"], 75);
        assert_eq!(counts["b"], 25);
    }

    #[test]
    fn zero_weight_treated_as_one() {
        let lb = LoadBalancer::new(vec![pool("a", 0)]);
        assert_eq!(lb.next().unwrap().addr, "a");
    }
}
