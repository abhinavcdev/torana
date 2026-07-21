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
    /// Automatic HTTPS via ACME (RFC 8555), instead of a static
    /// tls_cert/tls_key pair. Requires building with `--features acme`.
    /// Not combinable with tls_client_ca (mTLS) in this version.
    pub acme: Option<AutoTlsConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutoTlsConfig {
    pub domains: Vec<String>,
    pub contact_emails: Option<Vec<String>>,
    /// Where issued certificates/account keys are cached. Default
    /// "./acme-cache".
    pub cache_dir: Option<String>,
    /// Overrides the ACME directory URL entirely (e.g. a local test server
    /// like Pebble). Takes priority over `staging` when both are set.
    pub directory_url: Option<String>,
    /// Use Let's Encrypt's staging directory (untrusted test certs, no
    /// meaningful rate limits) instead of production. Default false.
    pub staging: Option<bool>,
    /// Trust this CA (PEM) when connecting to the ACME directory itself —
    /// for a private/internal ACME server, or a local test server like
    /// Pebble. The public web PKI roots are trusted when unset.
    pub ca_cert: Option<String>,
}

impl AutoTlsConfig {
    pub fn cache_dir(&self) -> &str {
        self.cache_dir.as_deref().unwrap_or("./acme-cache")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteConfig {
    pub name: String,
    /// Exact hostname match against the request's Host header (port
    /// stripped). A leading "*." matches that domain and all subdomains.
    /// `None` matches any host.
    pub host: Option<String>,
    /// Path prefix match, on segment boundaries ("/api" matches "/api" and
    /// "/api/v1" but not "/apiary"). `None` matches any path.
    pub path: Option<String>,
    pub when: Option<String>, // CEL expression (not implemented yet)
    pub upstream: Vec<UpstreamConfig>,
    pub mirror: Option<MirrorConfig>,
    #[serde(default)]
    pub timeout: TimeoutConfig,
    #[serde(default)]
    pub headers: HeaderConfig,
    pub health_check: Option<HealthCheckConfig>,
    /// Max total attempts (including the first) across different upstreams
    /// when a connection to an upstream fails outright. Never retries once
    /// a request has been sent. Defaults to 2 when the route has more than
    /// one upstream, 1 otherwise.
    pub retries: Option<u32>,
    /// Path to a sandboxed WASM request filter, evaluated before proxying.
    /// Requires building with `--features plugins`.
    pub plugin: Option<String>,
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

/// Active health check for a route's upstreams. Absent means every upstream
/// is always considered healthy (today's behavior).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthCheckConfig {
    /// HTTP path probed on each upstream. Default "/".
    pub path: Option<String>,
    /// How often to probe. Default "10s".
    pub interval: Option<String>,
    /// Per-probe timeout. Default "2s".
    pub timeout: Option<String>,
}

impl HealthCheckConfig {
    pub fn path(&self) -> &str {
        self.path.as_deref().unwrap_or("/")
    }
    pub fn interval_duration(&self) -> anyhow::Result<Duration> {
        self.interval
            .as_deref()
            .map_or(Ok(Duration::from_secs(10)), parse_duration)
    }
    pub fn timeout_duration(&self) -> anyhow::Result<Duration> {
        self.timeout
            .as_deref()
            .map_or(Ok(Duration::from_secs(2)), parse_duration)
    }
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

impl RouteConfig {
    /// Max total attempts across upstreams for this route.
    pub fn max_attempts(&self) -> u32 {
        match self.retries {
            Some(n) => n.max(1),
            None if self.upstream.len() > 1 => 2,
            None => 1,
        }
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
                "http" => {
                    if listener.acme.is_some() {
                        anyhow::bail!(
                            "listener {}: acme requires protocol = \"https\"",
                            listener.addr
                        );
                    }
                }
                "https" => match &listener.acme {
                    Some(acme) => {
                        if listener.tls_cert.is_some() || listener.tls_key.is_some() {
                            anyhow::bail!(
                                "listener {}: acme is mutually exclusive with tls_cert/tls_key",
                                listener.addr
                            );
                        }
                        if listener.tls_client_ca.is_some() {
                            anyhow::bail!(
                                "listener {}: acme combined with tls_client_ca (mTLS) is not \
                                 supported yet",
                                listener.addr
                            );
                        }
                        if acme.domains.is_empty() {
                            anyhow::bail!(
                                "listener {}: acme.domains must list at least one domain",
                                listener.addr
                            );
                        }
                        if let Some(url) = &acme.directory_url {
                            url.parse::<hyper::Uri>().map_err(|e| {
                                anyhow::anyhow!(
                                    "listener {}: invalid acme.directory_url: {}",
                                    listener.addr,
                                    e
                                )
                            })?;
                        }
                        if cfg!(not(feature = "acme")) {
                            anyhow::bail!(
                                "listener {}: acme is set but this build was compiled without \
                                 --features acme",
                                listener.addr
                            );
                        }
                    }
                    None => {
                        if listener.tls_cert.is_none() || listener.tls_key.is_none() {
                            anyhow::bail!(
                                "HTTPS listener {} requires tls_cert and tls_key (or an acme block)",
                                listener.addr
                            );
                        }
                    }
                },
                other => anyhow::bail!(
                    "Unsupported listener protocol '{}' (expected \"http\" or \"https\")",
                    other
                ),
            }
            if listener.tls_client_ca.is_some() && listener.protocol != "https" {
                anyhow::bail!(
                    "listener {}: tls_client_ca requires protocol = \"https\"",
                    listener.addr
                );
            }
        }

        for route in &self.route {
            if route.upstream.is_empty() {
                anyhow::bail!("Route '{}' has no upstreams", route.name);
            }
            if let Some(path) = &route.path {
                if !path.starts_with('/') {
                    anyhow::bail!(
                        "Route '{}': path '{}' must start with '/'",
                        route.name,
                        path
                    );
                }
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
            if let Some(hc) = &route.health_check {
                hc.interval_duration().map_err(|e| {
                    anyhow::anyhow!(
                        "Route '{}': invalid health_check.interval: {}",
                        route.name,
                        e
                    )
                })?;
                hc.timeout_duration().map_err(|e| {
                    anyhow::anyhow!(
                        "Route '{}': invalid health_check.timeout: {}",
                        route.name,
                        e
                    )
                })?;
            }
            if let Some(n) = route.retries {
                if n == 0 {
                    anyhow::bail!("Route '{}': retries must be at least 1", route.name);
                }
            }
            if route.plugin.is_some() && cfg!(not(feature = "plugins")) {
                anyhow::bail!(
                    "Route '{}': plugin is set but this build was compiled without \
                     --features plugins",
                    route.name
                );
            }
            if route.when.is_some() {
                tracing::warn!(
                    "Route '{}': 'when' expressions are not implemented yet and will be ignored",
                    route.name
                );
            }
            if let Some(mirror) = &route.mirror {
                crate::proxy::upstream_host_port(&mirror.addr).map_err(|e| {
                    anyhow::anyhow!(
                        "Route '{}': invalid mirror.addr '{}': {}",
                        route.name,
                        mirror.addr,
                        e
                    )
                })?;
                if let Some(rate) = mirror.rate {
                    if rate > 100 {
                        anyhow::bail!(
                            "Route '{}': mirror.rate must be 0-100, got {}",
                            route.name,
                            rate
                        );
                    }
                }
            }
            for (name, value) in route
                .headers
                .request
                .iter()
                .chain(route.headers.response.iter())
                .flatten()
            {
                hyper::header::HeaderName::from_bytes(name.as_bytes()).map_err(|e| {
                    anyhow::anyhow!(
                        "Route '{}': invalid header name '{}': {}",
                        route.name,
                        name,
                        e
                    )
                })?;
                if !value.is_empty() {
                    hyper::header::HeaderValue::from_str(value).map_err(|e| {
                        anyhow::anyhow!(
                            "Route '{}': invalid header value for '{}': {}",
                            route.name,
                            name,
                            e
                        )
                    })?;
                }
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
    fn rejects_path_without_leading_slash() {
        let mut config = minimal_config("http://127.0.0.1:9000");
        config.route[0].path = Some("api".into());
        assert!(config.validate().is_err());
    }

    #[test]
    fn rejects_zero_retries() {
        let mut config = minimal_config("http://127.0.0.1:9000");
        config.route[0].retries = Some(0);
        assert!(config.validate().is_err());
    }

    #[test]
    fn max_attempts_defaults() {
        let mut config = minimal_config("http://127.0.0.1:9000");
        assert_eq!(config.route[0].max_attempts(), 1);
        config.route[0].upstream.push(UpstreamConfig {
            addr: "http://127.0.0.1:9001".into(),
            weight: None,
        });
        assert_eq!(config.route[0].max_attempts(), 2);
        config.route[0].retries = Some(5);
        assert_eq!(config.route[0].max_attempts(), 5);
    }

    #[test]
    fn rejects_tls_client_ca_on_http_listener() {
        let mut config = minimal_config("http://127.0.0.1:9000");
        config.listener[0].tls_client_ca = Some("./ca.pem".into());
        assert!(config.validate().is_err());
    }

    fn acme_config(domains: &[&str]) -> AutoTlsConfig {
        AutoTlsConfig {
            domains: domains.iter().map(|s| s.to_string()).collect(),
            contact_emails: None,
            cache_dir: None,
            directory_url: None,
            staging: Some(true),
            ca_cert: None,
        }
    }

    #[test]
    fn acme_requires_https_protocol() {
        let mut config = minimal_config("http://127.0.0.1:9000");
        config.listener[0].acme = Some(acme_config(&["example.com"]));
        assert!(config.validate().is_err());
    }

    #[test]
    fn acme_rejects_empty_domains() {
        let mut config = minimal_config("http://127.0.0.1:9000");
        config.listener[0].protocol = "https".into();
        config.listener[0].acme = Some(acme_config(&[]));
        assert!(config.validate().is_err());
    }

    #[test]
    fn acme_mutually_exclusive_with_static_cert() {
        let mut config = minimal_config("http://127.0.0.1:9000");
        config.listener[0].protocol = "https".into();
        config.listener[0].tls_cert = Some("./tls.crt".into());
        config.listener[0].tls_key = Some("./tls.key".into());
        config.listener[0].acme = Some(acme_config(&["example.com"]));
        assert!(config.validate().is_err());
    }

    #[test]
    fn acme_mutually_exclusive_with_mtls() {
        let mut config = minimal_config("http://127.0.0.1:9000");
        config.listener[0].protocol = "https".into();
        config.listener[0].tls_client_ca = Some("./ca.pem".into());
        config.listener[0].acme = Some(acme_config(&["example.com"]));
        assert!(config.validate().is_err());
    }

    #[test]
    fn acme_rejects_invalid_directory_url() {
        let mut config = minimal_config("http://127.0.0.1:9000");
        config.listener[0].protocol = "https".into();
        let mut acme = acme_config(&["example.com"]);
        acme.directory_url = Some("not a url".into());
        config.listener[0].acme = Some(acme);
        assert!(config.validate().is_err());
    }

    #[test]
    #[cfg(feature = "acme")]
    fn acme_valid_config_passes_with_feature_enabled() {
        let mut config = minimal_config("http://127.0.0.1:9000");
        config.listener[0].protocol = "https".into();
        config.listener[0].acme = Some(acme_config(&["example.com"]));
        config.validate().unwrap();
    }

    #[test]
    #[cfg(not(feature = "acme"))]
    fn acme_valid_config_rejected_without_feature() {
        let mut config = minimal_config("http://127.0.0.1:9000");
        config.listener[0].protocol = "https".into();
        config.listener[0].acme = Some(acme_config(&["example.com"]));
        let err = config.validate().unwrap_err().to_string();
        assert!(err.contains("--features acme"), "unexpected error: {}", err);
    }

    #[test]
    fn rejects_invalid_mirror_addr() {
        let mut config = minimal_config("http://127.0.0.1:9000");
        config.route[0].mirror = Some(MirrorConfig {
            addr: "http://127.0.0.1:not-a-port".into(),
            rate: None,
        });
        assert!(config.validate().is_err());
    }

    #[test]
    fn rejects_mirror_rate_over_100() {
        let mut config = minimal_config("http://127.0.0.1:9000");
        config.route[0].mirror = Some(MirrorConfig {
            addr: "http://127.0.0.1:9001".into(),
            rate: Some(101),
        });
        assert!(config.validate().is_err());
    }

    #[test]
    fn accepts_valid_mirror() {
        let mut config = minimal_config("http://127.0.0.1:9000");
        config.route[0].mirror = Some(MirrorConfig {
            addr: "http://127.0.0.1:9001".into(),
            rate: Some(50),
        });
        config.validate().unwrap();
    }

    #[test]
    fn rejects_invalid_header_name() {
        let mut config = minimal_config("http://127.0.0.1:9000");
        let mut headers = HashMap::new();
        headers.insert("bad header name".to_string(), "value".to_string());
        config.route[0].headers.request = Some(headers);
        assert!(config.validate().is_err());
    }

    #[test]
    fn accepts_empty_header_value_as_removal() {
        let mut config = minimal_config("http://127.0.0.1:9000");
        let mut headers = HashMap::new();
        headers.insert("x-remove-me".to_string(), "".to_string());
        config.route[0].headers.response = Some(headers);
        config.validate().unwrap();
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
