use crate::config::{load_config, Config};
use crate::health::HealthRegistry;
use crate::upstream::{build_lb_map, LoadBalancer};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};
use tokio::task::JoinHandle;

pub type LbCache = Arc<RwLock<HashMap<String, Arc<LoadBalancer>>>>;
#[cfg(feature = "plugins")]
pub type PluginCache = Arc<RwLock<HashMap<String, Arc<crate::plugin::Plugin>>>>;

/// Everything a SIGHUP reload rebuilds in lockstep, so a request never sees
/// a route pointing at stale upstreams, health state, or plugins.
pub struct ReloadTargets {
    pub config_handle: Arc<RwLock<Config>>,
    pub lb_cache: LbCache,
    pub health_registry: HealthRegistry,
    pub health_handles: Arc<Mutex<Vec<JoinHandle<()>>>>,
    #[cfg(feature = "plugins")]
    pub plugin_cache: PluginCache,
}

pub async fn watch_config_signal(config_path: String, targets: ReloadTargets) {
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

        // Compile plugins (if any) before touching any running state: a
        // broken plugin must abort the whole reload, not half-apply it.
        #[cfg(feature = "plugins")]
        let new_plugins = match crate::plugin::build_plugin_cache(&new_config) {
            Ok(plugins) => plugins,
            Err(e) => {
                tracing::error!("Reload failed, keeping current config: {:#}", e);
                continue;
            }
        };

        let new_lbs = build_lb_map(&new_config);
        let new_health_handles =
            crate::health::spawn_all_probers(&new_config, targets.health_registry.clone());

        // Swap everything, then stop the old probers.
        {
            let mut cache = targets.lb_cache.write().await;
            *cache = new_lbs;
        }
        #[cfg(feature = "plugins")]
        {
            let mut cache = targets.plugin_cache.write().await;
            *cache = new_plugins;
        }
        {
            let mut handle = targets.config_handle.write().await;
            *handle = new_config;
        }
        {
            let mut old_handles = targets.health_handles.lock().await;
            let previous = std::mem::replace(&mut *old_handles, new_health_handles);
            for h in previous {
                h.abort();
            }
        }

        tracing::info!("Config reloaded successfully");
    }
}
