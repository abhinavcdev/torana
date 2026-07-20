use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::Duration;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub global: GlobalConfig,
    pub listener: Vec<ListenerConfig>,
    pub route: Vec<RouteConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GlobalConfig {
    pub workers: Option<String>,
    pub log_format: Option<String>,   // "json" or "text"
    pub log_level: Option<String>,    // "info", "debug", "warn"
    pub metrics_addr: Option<String>, // "127.0.0.1:9090"
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListenerConfig {
    pub addr: String,
    pub protocol: String, // "http", "https"
    pub tls_cert: Option<String>,
    pub tls_key: Option<String>,
    pub tls_client_ca: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteConfig {
    pub name: String,
    pub when: Option<String>, // CEL expression (not implemented yet)
    pub upstream: Vec<UpstreamConfig>,
    pub mirror: Option<MirrorConfig>,
    #[serde(default)]
    pub timeout: TimeoutConfig,
    #[serde(default)]
    pub headers: HeaderConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpstreamConfig {
    pub addr: String,
    pub weight: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MirrorConfig {
    pub addr: String,
    pub rate: Option<u32>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TimeoutConfig {
    pub connect: Option<String>,
    pub first_byte: Option<String>,
    pub total: Option<String>,
}

impl TimeoutConfig {
    /// Total request timeout (time until the upstream returns response
    /// headers). Invalid values are rejected during `validate`.
    pub fn total_duration(&self) -> Option<Duration> {
        self.total.as_deref().and_then(|s| parse_duration(s).ok())
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HeaderConfig {
    pub request: Option<HashMap<String, String>>,
    pub response: Option<HashMap<String, String>>,
}

pub fn load_config(path: &str) -> anyhow::Result<Config> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("Failed to read config file {}: {}", path, e))?;
    let config: Config = toml::from_str(&content)?;
    Ok(config)
}

/// Parse durations like "200ms", "30s", "5m".
pub fn parse_duration(s: &str) -> anyhow::Result<Duration> {
    let s = s.trim();
    let (num, unit) = s
        .find(|c: char| !c.is_ascii_digit())
        .map(|i| s.split_at(i))
        .ok_or_else(|| anyhow::anyhow!("Duration '{}' is missing a unit (ms, s, m)", s))?;
    let value: u64 = num
        .parse()
        .map_err(|_| anyhow::anyhow!("Invalid duration value '{}'", s))?;
    match unit {
        "ms" => Ok(Duration::from_millis(value)),
        "s" => Ok(Duration::from_secs(value)),
        "m" => Ok(Duration::from_secs(value * 60)),
        _ => anyhow::bail!("Unknown duration unit '{}' in '{}'", unit, s),
    }
}

impl Config {
    /// Reject configs that cannot work and warn about accepted-but-ignored
    /// fields so users aren't silently surprised.
    pub fn validate(&self) -> anyhow::Result<()> {
        if self.listener.is_empty() {
            anyhow::bail!("Config must define at least one [[listener]]");
        }
        if self.route.is_empty() {
            anyhow::bail!("Config must define at least one [[route]]");
        }

        for listener in &self.listener {
            listener
                .addr
                .parse::<std::net::SocketAddr>()
                .map_err(|e| anyhow::anyhow!("Invalid listener addr '{}': {}", listener.addr, e))?;
            match listener.protocol.as_str() {
                "http" => {}
                "https" => {
                    if listener.tls_cert.is_none() || listener.tls_key.is_none() {
                        anyhow::bail!(
                            "HTTPS listener {} requires tls_cert and tls_key",
                            listener.addr
                        );
                    }
                }
                other => anyhow::bail!(
                    "Unsupported listener protocol '{}' (expected \"http\" or \"https\")",
                    other
                ),
            }
            if listener.tls_client_ca.is_some() {
                tracing::warn!(
                    "listener {}: tls_client_ca (mTLS) is not implemented yet and will be ignored",
                    listener.addr
                );
            }
        }

        if self.route.len() > 1 {
            tracing::warn!(
                "{} routes configured but request matching is not implemented yet; \
                 only the first route '{}' will receive traffic",
                self.route.len(),
                self.route[0].name
            );
        }

        for route in &self.route {
            if route.upstream.is_empty() {
                anyhow::bail!("Route '{}' has no upstreams", route.name);
            }
            for upstream in &route.upstream {
                if upstream.addr.starts_with("https://") {
                    anyhow::bail!(
                        "Route '{}': upstream '{}' uses https:// but TLS to upstreams \
                         is not implemented yet; use an http:// upstream",
                        route.name,
                        upstream.addr
                    );
                }
                crate::proxy::upstream_host_port(&upstream.addr).map_err(|e| {
                    anyhow::anyhow!(
                        "Route '{}': invalid upstream '{}': {}",
                        route.name,
                        upstream.addr,
                        e
                    )
                })?;
            }
            if let Some(total) = route.timeout.total.as_deref() {
                parse_duration(total).map_err(|e| {
                    anyhow::anyhow!("Route '{}': invalid timeout.total: {}", route.name, e)
                })?;
            }
            if route.when.is_some() {
                tracing::warn!(
                    "Route '{}': 'when' expressions are not implemented yet and will be ignored",
                    route.name
                );
            }
            if route.mirror.is_some() {
                tracing::warn!(
                    "Route '{}': 'mirror' is not implemented yet and will be ignored",
                    route.name
                );
            }
            if route.headers.request.is_some() || route.headers.response.is_some() {
                tracing::warn!(
                    "Route '{}': header rewriting is not implemented yet and will be ignored",
                    route.name
                );
            }
            if route.timeout.connect.is_some() || route.timeout.first_byte.is_some() {
                tracing::warn!(
                    "Route '{}': only timeout.total is enforced; connect/first_byte are ignored",
                    route.name
                );
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal_config(upstream_addr: &str) -> Config {
        toml::from_str(&format!(
            r#"
            [global]

            [[listener]]
            addr = "127.0.0.1:8080"
            protocol = "http"

            [[route]]
            name = "default"
            upstream = [{{ addr = "{}" }}]
            "#,
            upstream_addr
        ))
        .unwrap()
    }

    #[test]
    fn parses_minimal_config() {
        let config = minimal_config("http://127.0.0.1:9000");
        assert_eq!(config.listener.len(), 1);
        assert_eq!(config.route[0].upstream[0].addr, "http://127.0.0.1:9000");
        config.validate().unwrap();
    }

    #[test]
    fn rejects_https_upstream() {
        let config = minimal_config("https://example.com");
        let err = config.validate().unwrap_err().to_string();
        assert!(err.contains("not implemented"), "unexpected error: {}", err);
    }

    #[test]
    fn rejects_https_listener_without_cert() {
        let mut config = minimal_config("http://127.0.0.1:9000");
        config.listener[0].protocol = "https".into();
        assert!(config.validate().is_err());
    }

    #[test]
    fn rejects_unknown_protocol() {
        let mut config = minimal_config("http://127.0.0.1:9000");
        config.listener[0].protocol = "quic".into();
        assert!(config.validate().is_err());
    }

    #[test]
    fn parse_duration_units() {
        assert_eq!(parse_duration("200ms").unwrap(), Duration::from_millis(200));
        assert_eq!(parse_duration("30s").unwrap(), Duration::from_secs(30));
        assert_eq!(parse_duration("5m").unwrap(), Duration::from_secs(300));
        assert!(parse_duration("30").is_err());
        assert!(parse_duration("abc").is_err());
        assert!(parse_duration("30h").is_err());
    }
}
