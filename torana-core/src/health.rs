use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::RwLock;

/// Shared health state for upstreams, keyed by address. An upstream with no
/// active health check configured is simply never inserted here, and reads
/// as healthy — that preserves today's behavior for routes that don't opt
/// in to health checking.
#[derive(Clone, Default)]
pub struct HealthRegistry {
    state: Arc<RwLock<HashMap<String, Arc<AtomicBool>>>>,
}

impl HealthRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn is_healthy(&self, addr: &str) -> bool {
        match self.state.read().await.get(addr) {
            Some(flag) => flag.load(Ordering::Relaxed),
            None => true,
        }
    }

    pub(crate) async fn set(&self, addr: &str, healthy: bool) {
        let flag = {
            let map = self.state.read().await;
            map.get(addr).cloned()
        };
        let flag = match flag {
            Some(f) => f,
            None => {
                let mut map = self.state.write().await;
                map.entry(addr.to_string())
                    .or_insert_with(|| Arc::new(AtomicBool::new(true)))
                    .clone()
            }
        };
        let was_healthy = flag.swap(healthy, Ordering::Relaxed);
        if was_healthy != healthy {
            if healthy {
                tracing::info!(upstream = addr, "upstream became healthy");
            } else {
                tracing::warn!(upstream = addr, "upstream became unhealthy");
            }
        }
    }
}

/// Spawns a background prober for one upstream that runs for the lifetime
/// of the process (or until the task is aborted on reload).
pub fn spawn_prober(
    addr: String,
    path: String,
    interval: Duration,
    timeout: Duration,
    registry: HealthRegistry,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            let healthy = probe_once(&addr, &path, timeout).await;
            registry.set(&addr, healthy).await;
            tokio::time::sleep(interval).await;
        }
    })
}

/// Spawn a prober for every upstream on every route that opts into
/// `health_check`. Called at startup and again on every successful reload.
pub fn spawn_all_probers(
    config: &crate::config::Config,
    registry: HealthRegistry,
) -> Vec<tokio::task::JoinHandle<()>> {
    let mut handles = Vec::new();
    for route in &config.route {
        let Some(hc) = &route.health_check else {
            continue;
        };
        let interval = hc.interval_duration().unwrap_or(Duration::from_secs(10));
        let timeout = hc.timeout_duration().unwrap_or(Duration::from_secs(2));
        let path = hc.path().to_string();
        for upstream in &route.upstream {
            handles.push(spawn_prober(
                upstream.addr.clone(),
                path.clone(),
                interval,
                timeout,
                registry.clone(),
            ));
        }
    }
    handles
}

async fn probe_once(addr: &str, path: &str, timeout: Duration) -> bool {
    let Ok((host, port)) = crate::proxy::upstream_host_port(addr) else {
        return false;
    };
    let attempt = async move {
        let mut stream = tokio::net::TcpStream::connect((host.as_str(), port)).await?;
        let request = format!(
            "GET {} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
            path, host
        );
        stream.write_all(request.as_bytes()).await?;

        let mut buf = [0u8; 512];
        let n = stream.read(&mut buf).await?;
        let head = String::from_utf8_lossy(&buf[..n]);
        let status: u16 = head
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        Ok::<bool, std::io::Error>(status != 0 && status < 500)
    };
    tokio::time::timeout(timeout, attempt)
        .await
        .map(|r| r.unwrap_or(false))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn untracked_addr_is_healthy() {
        let registry = HealthRegistry::new();
        assert!(registry.is_healthy("http://127.0.0.1:1").await);
    }

    #[tokio::test]
    async fn set_then_read_reflects_state() {
        let registry = HealthRegistry::new();
        registry.set("http://127.0.0.1:1", false).await;
        assert!(!registry.is_healthy("http://127.0.0.1:1").await);
        registry.set("http://127.0.0.1:1", true).await;
        assert!(registry.is_healthy("http://127.0.0.1:1").await);
    }

    #[tokio::test]
    async fn probe_unreachable_upstream_is_unhealthy() {
        // Port 1 is a reserved low port almost never listening; treat a
        // refused/failed connection as unhealthy.
        let healthy = probe_once("http://127.0.0.1:1", "/", Duration::from_millis(300)).await;
        assert!(!healthy);
    }
}
