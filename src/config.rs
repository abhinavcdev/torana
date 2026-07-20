use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub global: GlobalConfig,
    pub listener: Vec<ListenerConfig>,
    pub route: Vec<RouteConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GlobalConfig {
    pub workers: Option<String>,
    pub log_format: Option<String>,  // "json" or "logfmt"
    pub log_level: Option<String>,   // "info", "debug", "warn"
    pub metrics_addr: Option<String>, // "0.0.0.0:9090"
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListenerConfig {
    pub addr: String,
    pub protocol: String,  // "http", "https"
    pub tls_cert: Option<String>,
    pub tls_key: Option<String>,
    pub tls_client_ca: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteConfig {
    pub name: String,
    pub when: Option<String>,  // CEL expression (v0.2+)
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

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HeaderConfig {
    pub request: Option<HashMap<String, String>>,
    pub response: Option<HashMap<String, String>>,
}

pub fn load_config(path: &str) -> anyhow::Result<Config> {
    let content = std::fs::read_to_string(path)?;
    let config: Config = toml::from_str(&content)?;
    Ok(config)
}
