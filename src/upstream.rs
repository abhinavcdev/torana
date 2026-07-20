use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Debug, Clone)]
pub struct UpstreamPool {
    pub addr: String,
    pub weight: u32,
}

pub struct LoadBalancer {
    upstreams: Vec<UpstreamPool>,
    counter: Arc<AtomicUsize>,
}

impl LoadBalancer {
    pub fn new(upstreams: Vec<UpstreamPool>) -> Self {
        // Expand upstreams based on weights with interleaving
        // Example: weight=50, weight=50 becomes [upstream1, upstream2, upstream1, upstream2, ...]
        let max_weight = upstreams.iter().map(|u| u.weight).max().unwrap_or(1);
        let mut expanded = Vec::new();

        for i in 0..max_weight {
            for u in upstreams.iter() {
                if i < u.weight {
                    expanded.push(u.clone());
                }
            }
        }

        LoadBalancer {
            upstreams: expanded,
            counter: Arc::new(AtomicUsize::new(0)),
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
