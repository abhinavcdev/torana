mod config;
mod logging;
mod metrics;
mod upstream;
mod proxy;
mod tls;
mod reload;

use clap::Parser;
use config::load_config;
use metrics::init_metrics;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::RwLock;
use hyper::{Request, Response};
use hyper::service::service_fn;
use std::time::Duration;
use std::collections::HashMap;

#[derive(Parser)]
#[command(name = "caddyrs")]
#[command(about = "Rust-native micro reverse proxy")]
struct Cli {
    /// Path to config file
    #[arg(short, long, default_value = "caddy.rs.toml")]
    config: String,
}

pub async fn handle_shutdown() {
    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .expect("Failed to install SIGTERM handler");

    sigterm.recv().await;

    tracing::info!("SIGTERM received, initiating graceful shutdown");

    // Set flag or log, then exit after timeout
    let shutdown_timeout = Duration::from_secs(30);
    tokio::time::timeout(shutdown_timeout, async {
        // Wait for in-flight requests (simplified for v0.1)
    }).await.ok();

    tracing::info!("Shutdown complete");
    std::process::exit(0);
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let config = load_config(&cli.config)?;

    let log_level = config.global.log_level.as_deref().unwrap_or("info");
    let log_format = config.global.log_format.as_deref().unwrap_or("json");
    logging::init_logging(log_format, log_level)?;

    tracing::info!("caddyrs starting");

    // Create config handle for zero-downtime reload
    let config_handle: Arc<RwLock<config::Config>> = Arc::new(RwLock::new(config.clone()));

    // Spawn config reload watcher
    let config_handle_clone = config_handle.clone();
    let config_path = cli.config.clone();
    tokio::spawn(async move {
        reload::watch_config_signal(config_path, config_handle_clone).await
    });

    let (_metrics, registry) = init_metrics()?;
    let metrics_addr = config.global.metrics_addr.as_deref().unwrap_or("127.0.0.1:9090").to_string();

    // Spawn metrics server
    let registry_clone = registry.clone();
    let metrics_addr_clone = metrics_addr.clone();
    tokio::spawn(async move {
        let listener = tokio::net::TcpListener::bind(&metrics_addr_clone)
            .await
            .expect("Failed to bind metrics addr");

        loop {
            let (mut stream, _) = listener.accept().await.expect("Accept failed");
            let registry = registry_clone.clone();

            tokio::spawn(async move {
                use prometheus::TextEncoder;
                use prometheus::Encoder;
                use tokio::io::AsyncWriteExt;

                let encoder = TextEncoder::new();
                let metric_families = registry.gather();
                let mut buffer = Vec::new();
                encoder.encode(&metric_families, &mut buffer).unwrap_or_default();

                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/plain; version=0.0.4\r\nContent-Length: {}\r\n\r\n{}",
                    buffer.len(),
                    String::from_utf8_lossy(&buffer)
                );

                let _ = stream.write_all(response.as_bytes()).await;
                let _ = stream.flush().await;
            });
        }
    });

    tracing::info!("Metrics server listening on {}", metrics_addr);

    let metrics_clone = _metrics.clone();
    let config_handle_listeners = config_handle.clone();

    // Create load balancer cache - one LoadBalancer instance per route
    let lb_cache: Arc<RwLock<HashMap<String, Arc<upstream::LoadBalancer>>>> = Arc::new(RwLock::new(HashMap::new()));

    // Initialize load balancers for current routes
    {
        let config_guard = config_handle_listeners.read().await;
        let mut cache = lb_cache.write().await;
        for route in &config_guard.route {
            let upstreams: Vec<upstream::UpstreamPool> = route
                .upstream
                .iter()
                .map(|u| upstream::UpstreamPool {
                    addr: u.addr.clone(),
                    weight: u.weight.unwrap_or(100),
                })
                .collect();
            cache.insert(route.name.clone(), Arc::new(upstream::LoadBalancer::new(upstreams)));
        }
        drop(cache);
        drop(config_guard);
    }

    // Spawn HTTP and HTTPS listeners
    {
        let config_guard = config_handle_listeners.read().await;
        for listener_cfg in &config_guard.listener {
            if listener_cfg.protocol == "http" {
                let addr = listener_cfg.addr.parse::<SocketAddr>()?;
                let config_handle = config_handle_listeners.clone();
                let metrics = metrics_clone.clone();
                let lb_cache = lb_cache.clone();

                tokio::spawn(async move {
                    match start_http_listener(addr, config_handle, metrics, lb_cache).await {
                        Ok(_) => {}
                        Err(e) => tracing::error!("HTTP listener error: {}", e),
                    }
                });
            } else if listener_cfg.protocol == "https" {
                let addr = listener_cfg.addr.parse::<SocketAddr>()?;
                let cert_path = listener_cfg.tls_cert.clone().ok_or_else(|| {
                    anyhow::anyhow!("HTTPS listener requires tls_cert")
                })?;
                let key_path = listener_cfg.tls_key.clone().ok_or_else(|| {
                    anyhow::anyhow!("HTTPS listener requires tls_key")
                })?;
                let config_handle = config_handle_listeners.clone();
                let metrics = metrics_clone.clone();
                let lb_cache = lb_cache.clone();

                tokio::spawn(async move {
                    match start_https_listener(addr, cert_path, key_path, config_handle, metrics, lb_cache).await {
                        Ok(_) => {}
                        Err(e) => tracing::error!("HTTPS listener error: {}", e),
                    }
                });
            }
        }
        drop(config_guard);
    }

    tracing::info!("caddyrs ready");

    // Spawn graceful shutdown handler
    tokio::spawn(handle_shutdown());

    // Keep the main task alive indefinitely
    std::future::pending::<()>().await;

    #[allow(unreachable_code)]
    Ok(())
}

async fn start_http_listener(
    addr: SocketAddr,
    config_handle: Arc<RwLock<config::Config>>,
    metrics: metrics::Metrics,
    lb_cache: Arc<RwLock<HashMap<String, Arc<upstream::LoadBalancer>>>>,
) -> anyhow::Result<()> {
    let metrics = std::sync::Arc::new(metrics);

    let listener = tokio::net::TcpListener::bind(&addr).await?;

    tracing::info!("HTTP listener started on {}", addr);

    loop {
        let (stream, _) = listener.accept().await?;
        let config_handle = config_handle.clone();
        let metrics = metrics.clone();
        let lb_cache = lb_cache.clone();

        tokio::spawn(async move {
            use hyper_util::rt::TokioIo;

            let io = TokioIo::new(stream);
            let service = service_fn(move |req: Request<hyper::body::Incoming>| {
                let config_handle = config_handle.clone();
                let metrics = metrics.clone();
                let lb_cache = lb_cache.clone();
                handle_request(req, config_handle, metrics, lb_cache)
            });

            if let Err(e) = hyper::server::conn::http1::Builder::new()
                .serve_connection(io, service)
                .await
            {
                tracing::error!("Connection error: {}", e);
            }
        });
    }
}

async fn start_https_listener(
    addr: SocketAddr,
    cert_path: String,
    key_path: String,
    config_handle: Arc<RwLock<config::Config>>,
    metrics: metrics::Metrics,
    lb_cache: Arc<RwLock<HashMap<String, Arc<upstream::LoadBalancer>>>>,
) -> anyhow::Result<()> {
    let tls_acceptor = tls::load_tls_config(&cert_path, &key_path)?;
    let metrics = std::sync::Arc::new(metrics);

    let listener = tokio::net::TcpListener::bind(&addr).await?;

    tracing::info!("HTTPS listener started on {}", addr);

    loop {
        let (stream, _) = listener.accept().await?;
        let tls_acceptor = tls_acceptor.clone();
        let config_handle = config_handle.clone();
        let metrics = metrics.clone();
        let lb_cache = lb_cache.clone();

        tokio::spawn(async move {
            match tls_acceptor.accept(stream).await {
                Ok(tls_stream) => {
                    use hyper_util::rt::TokioIo;

                    let io = TokioIo::new(tls_stream);
                    let service = service_fn(move |req: Request<hyper::body::Incoming>| {
                        let config_handle = config_handle.clone();
                        let metrics = metrics.clone();
                        let lb_cache = lb_cache.clone();
                        handle_request(req, config_handle, metrics, lb_cache)
                    });

                    if let Err(e) = hyper::server::conn::http1::Builder::new()
                        .serve_connection(io, service)
                        .await
                    {
                        tracing::error!("HTTPS connection error: {}", e);
                    }
                }
                Err(e) => {
                    tracing::warn!("TLS error: {}", e);
                }
            }
        });
    }
}

async fn handle_request(
    req: Request<hyper::body::Incoming>,
    config_handle: Arc<RwLock<config::Config>>,
    metrics: std::sync::Arc<metrics::Metrics>,
    lb_cache: Arc<RwLock<HashMap<String, Arc<upstream::LoadBalancer>>>>,
) -> Result<Response<http_body_util::Full<bytes::Bytes>>, hyper::Error> {
    metrics.http_requests_total.inc();

    // Read routes from CURRENT config (respects SIGHUP reloads)
    let config_guard = config_handle.read().await;
    let routes = &config_guard.route;

    if routes.is_empty() {
        tracing::warn!("No routes configured");
        return Ok(Response::builder()
            .status(502)
            .body(http_body_util::Full::new(bytes::Bytes::from_static(b"Bad Gateway: No routes configured")))
            .unwrap());
    }

    let route = &routes[0];

    if route.upstream.is_empty() {
        tracing::warn!("Route has no upstreams");
        return Ok(Response::builder()
            .status(502)
            .body(http_body_util::Full::new(bytes::Bytes::from_static(b"Bad Gateway: No upstreams available")))
            .unwrap());
    }

    // Get cached LoadBalancer or create new one if route was updated
    let lb = {
        let cache = lb_cache.read().await;
        if let Some(lb) = cache.get(&route.name) {
            lb.clone()
        } else {
            drop(cache);
            // Route not in cache, create new LoadBalancer and add to cache
            let upstreams: Vec<upstream::UpstreamPool> = route
                .upstream
                .iter()
                .map(|u| upstream::UpstreamPool {
                    addr: u.addr.clone(),
                    weight: u.weight.unwrap_or(100),
                })
                .collect();
            let new_lb = Arc::new(upstream::LoadBalancer::new(upstreams));
            let mut cache_mut = lb_cache.write().await;
            cache_mut.insert(route.name.clone(), new_lb.clone());
            new_lb
        }
    };

    // Get next upstream via load balancer
    let upstream_addr = if let Some(upstream) = lb.next() {
        upstream.addr.clone()
    } else {
        drop(config_guard);
        return Ok(Response::builder()
            .status(502)
            .body(http_body_util::Full::new(bytes::Bytes::from_static(b"Bad Gateway: No upstreams available")))
            .unwrap());
    };

    drop(config_guard);

    tracing::debug!("Handling request: {} {}", req.method(), req.uri());

    match proxy::proxy_request(req, &upstream_addr).await {
        Ok(response) => {
            // Convert Incoming to Full<Bytes>
            let (parts, body) = response.into_parts();
            let body_bytes = http_body_util::BodyExt::collect(body).await.unwrap_or_default().to_bytes();
            let boxed_body = http_body_util::Full::new(body_bytes);
            Ok(Response::from_parts(parts, boxed_body))
        }
        Err(e) => {
            tracing::error!("Proxy request failed: {}", e);
            metrics.upstream_connection_errors.inc();
            Ok(Response::builder()
                .status(502)
                .body(http_body_util::Full::new(bytes::Bytes::from_static(b"Bad Gateway")))
                .unwrap())
        }
    }
}
