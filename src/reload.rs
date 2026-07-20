use crate::config::{load_config, Config};
use crate::upstream::{build_lb_map, LoadBalancer};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

pub async fn watch_config_signal(
    config_path: String,
    config_handle: Arc<RwLock<Config>>,
    lb_cache: Arc<RwLock<HashMap<String, Arc<LoadBalancer>>>>,
) {
    use tokio::signal::unix::{signal, SignalKind};

    let mut sighup = match signal(SignalKind::hangup()) {
        Ok(sig) => sig,
        Err(e) => {
            tracing::error!("Failed to install SIGHUP handler: {}", e);
            return;
        }
    };

    loop {
        sighup.recv().await;
        tracing::info!("SIGHUP received, reloading config");

        let new_config = match load_config(&config_path) {
            Ok(config) => config,
            Err(e) => {
                tracing::error!("Reload failed, keeping current config: {}", e);
                continue;
            }
        };
        if let Err(e) = new_config.validate() {
            tracing::error!("Reload failed, keeping current config: {}", e);
            continue;
        }

        // Rebuild load balancers before swapping the config so requests
        // never see a new route pointing at stale upstreams.
        let new_lbs = build_lb_map(&new_config);
        {
            let mut cache = lb_cache.write().await;
            *cache = new_lbs;
        }
        {
            let mut handle = config_handle.write().await;
            *handle = new_config;
        }
        tracing::info!("Config reloaded successfully");
    }
}
