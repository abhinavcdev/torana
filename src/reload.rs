use crate::config::{Config, load_config};
use std::sync::Arc;
use tokio::sync::RwLock;

pub async fn watch_config_signal(config_path: String, config_handle: Arc<RwLock<Config>>) {
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

        match load_config(&config_path) {
            Ok(new_config) => {
                let mut handle = config_handle.write().await;
                *handle = new_config;
                tracing::info!("Config reloaded successfully");
            }
            Err(e) => {
                tracing::error!("Failed to reload config: {}", e);
            }
        }
    }
}
