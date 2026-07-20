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

    /// Health- and retry-aware selection: skips upstreams in `exclude`
    /// (already tried this request) and prefers upstreams the health
    /// registry considers up. Falls back to a healthy-but-excluded upstream,
    /// and finally fails open (returns the next candidate regardless of
    /// health) rather than blackholing all traffic on a bad health check.
    pub async fn next_healthy(
        &self,
        health: &crate::health::HealthRegistry,
        exclude: &[String],
    ) -> Option<&UpstreamPool> {
        if self.upstreams.is_empty() {
            return None;
        }
        let len = self.upstreams.len();
        let start = self.counter.fetch_add(1, Ordering::Relaxed);

        for i in 0..len {
            let candidate = &self.upstreams[(start + i) % len];
            if !exclude.iter().any(|e| e == &candidate.addr)
                && health.is_healthy(&candidate.addr).await
            {
                return Some(candidate);
            }
        }
        for i in 0..len {
            let candidate = &self.upstreams[(start + i) % len];
            if health.is_healthy(&candidate.addr).await {
                return Some(candidate);
            }
        }
        Some(&self.upstreams[start % len])
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

    #[tokio::test]
    async fn next_healthy_skips_unhealthy() {
        let lb = LoadBalancer::new(vec![pool("a", 50), pool("b", 50)]);
        let health = crate::health::HealthRegistry::new();
        health.set("a", false).await;

        let picks: Vec<String> = {
            let mut v = vec![];
            for _ in 0..4 {
                v.push(lb.next_healthy(&health, &[]).await.unwrap().addr.clone());
            }
            v
        };
        assert!(
            picks.iter().all(|p| p == "b"),
            "expected only b, got {:?}",
            picks
        );
    }

    #[tokio::test]
    async fn next_healthy_skips_excluded() {
        let lb = LoadBalancer::new(vec![pool("a", 50), pool("b", 50)]);
        let health = crate::health::HealthRegistry::new();

        let picks: Vec<String> = {
            let mut v = vec![];
            for _ in 0..4 {
                v.push(
                    lb.next_healthy(&health, &["a".to_string()])
                        .await
                        .unwrap()
                        .addr
                        .clone(),
                );
            }
            v
        };
        assert!(
            picks.iter().all(|p| p == "b"),
            "expected only b, got {:?}",
            picks
        );
    }

    #[tokio::test]
    async fn next_healthy_fails_open_when_all_excluded() {
        let lb = LoadBalancer::new(vec![pool("a", 100)]);
        let health = crate::health::HealthRegistry::new();
        // Only upstream is excluded (already tried) but must still be
        // returned rather than yielding None.
        let result = lb.next_healthy(&health, &["a".to_string()]).await;
        assert_eq!(result.unwrap().addr, "a");
    }
}
