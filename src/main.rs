mod config;
mod logging;
mod metrics;
mod proxy;
mod reload;
mod tls;
mod upstream;

use bytes::Bytes;
use clap::Parser;
use config::load_config;
use http_body_util::combinators::BoxBody;
use http_body_util::{BodyExt, Full};
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use metrics::init_metrics;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

/// Applied when a route does not set timeout.total.
const DEFAULT_TOTAL_TIMEOUT: Duration = Duration::from_secs(30);

type ProxyBody = BoxBody<Bytes, hyper::Error>;
type LbCache = Arc<RwLock<HashMap<String, Arc<upstream::LoadBalancer>>>>;

#[derive(Parser)]
#[command(name = "caddyrs")]
#[command(about = "Rust-native micro reverse proxy")]
struct Cli {
    /// Path to config file
    #[arg(short, long, default_value = "caddy.rs.toml")]
    config: String,
}

async fn shutdown_signal() {
    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .expect("Failed to install SIGTERM handler");
    tokio::select! {
        _ = sigterm.recv() => tracing::info!("SIGTERM received, shutting down"),
        _ = tokio::signal::ctrl_c() => tracing::info!("SIGINT received, shutting down"),
    }
    // In-flight connections are dropped; connection draining is not
    // implemented yet.
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let config = load_config(&cli.config)?;

    let log_level = config.global.log_level.as_deref().unwrap_or("info");
    let log_format = config.global.log_format.as_deref().unwrap_or("json");
    logging::init_logging(log_format, log_level)?;

    config.validate()?;

    tracing::info!("caddyrs starting");

    let config_handle: Arc<RwLock<config::Config>> = Arc::new(RwLock::new(config.clone()));
    let lb_cache: LbCache = Arc::new(RwLock::new(upstream::build_lb_map(&config)));

    // SIGHUP triggers a config reload that also rebuilds the load balancers.
    tokio::spawn(reload::watch_config_signal(
        cli.config.clone(),
        config_handle.clone(),
        lb_cache.clone(),
    ));

    let (metrics, registry) = init_metrics()?;
    let metrics = Arc::new(metrics);

    let metrics_addr = config
        .global
        .metrics_addr
        .as_deref()
        .unwrap_or("127.0.0.1:9090")
        .to_string();
    let metrics_listener = tokio::net::TcpListener::bind(&metrics_addr)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to bind metrics addr {}: {}", metrics_addr, e))?;
    tokio::spawn(serve_metrics(metrics_listener, registry));
    tracing::info!("Metrics server listening on {}", metrics_addr);

    // Bind every listener before accepting traffic so a bad address or an
    // in-use port fails startup with a non-zero exit instead of a log line.
    for listener_cfg in &config.listener {
        let addr = listener_cfg.addr.parse::<SocketAddr>()?;
        let tcp = tokio::net::TcpListener::bind(addr)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to bind {}: {}", addr, e))?;

        let config_handle = config_handle.clone();
        let metrics = metrics.clone();
        let lb_cache = lb_cache.clone();

        if listener_cfg.protocol == "https" {
            let cert_path = listener_cfg.tls_cert.clone().expect("validated");
            let key_path = listener_cfg.tls_key.clone().expect("validated");
            let acceptor = tls::load_tls_config(&cert_path, &key_path)?;
            tracing::info!("HTTPS listener started on {}", addr);
            tokio::spawn(run_https_listener(
                tcp,
                acceptor,
                config_handle,
                metrics,
                lb_cache,
            ));
        } else {
            tracing::info!("HTTP listener started on {}", addr);
            tokio::spawn(run_http_listener(tcp, config_handle, metrics, lb_cache));
        }
    }

    tracing::info!("caddyrs ready");

    shutdown_signal().await;
    tracing::info!("Shutdown complete");
    Ok(())
}

async fn accept_or_retry(
    listener: &tokio::net::TcpListener,
) -> (tokio::net::TcpStream, SocketAddr) {
    loop {
        match listener.accept().await {
            Ok(conn) => return conn,
            Err(e) => {
                // Transient errors (EMFILE, ECONNABORTED) must not kill the
                // accept loop.
                tracing::warn!("Accept error: {}", e);
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }
    }
}

async fn run_http_listener(
    listener: tokio::net::TcpListener,
    config_handle: Arc<RwLock<config::Config>>,
    metrics: Arc<metrics::Metrics>,
    lb_cache: LbCache,
) {
    loop {
        let (stream, peer) = accept_or_retry(&listener).await;
        let config_handle = config_handle.clone();
        let metrics = metrics.clone();
        let lb_cache = lb_cache.clone();

        tokio::spawn(async move {
            let io = hyper_util::rt::TokioIo::new(stream);
            let service = service_fn(move |req: Request<hyper::body::Incoming>| {
                let config_handle = config_handle.clone();
                let metrics = metrics.clone();
                let lb_cache = lb_cache.clone();
                handle_request(req, peer, "http", config_handle, metrics, lb_cache)
            });

            if let Err(e) = hyper::server::conn::http1::Builder::new()
                .serve_connection(io, service)
                .await
            {
                tracing::debug!("Connection error: {}", e);
            }
        });
    }
}

async fn run_https_listener(
    listener: tokio::net::TcpListener,
    acceptor: tokio_rustls::TlsAcceptor,
    config_handle: Arc<RwLock<config::Config>>,
    metrics: Arc<metrics::Metrics>,
    lb_cache: LbCache,
) {
    loop {
        let (stream, peer) = accept_or_retry(&listener).await;
        let acceptor = acceptor.clone();
        let config_handle = config_handle.clone();
        let metrics = metrics.clone();
        let lb_cache = lb_cache.clone();

        tokio::spawn(async move {
            let tls_stream = match acceptor.accept(stream).await {
                Ok(s) => s,
                Err(e) => {
                    tracing::debug!("TLS handshake failed: {}", e);
                    return;
                }
            };

            let io = hyper_util::rt::TokioIo::new(tls_stream);
            let service = service_fn(move |req: Request<hyper::body::Incoming>| {
                let config_handle = config_handle.clone();
                let metrics = metrics.clone();
                let lb_cache = lb_cache.clone();
                handle_request(req, peer, "https", config_handle, metrics, lb_cache)
            });

            if let Err(e) = hyper::server::conn::http1::Builder::new()
                .serve_connection(io, service)
                .await
            {
                tracing::debug!("HTTPS connection error: {}", e);
            }
        });
    }
}

async fn serve_metrics(listener: tokio::net::TcpListener, registry: prometheus::Registry) {
    loop {
        let (mut stream, _) = accept_or_retry(&listener).await;
        let registry = registry.clone();

        tokio::spawn(async move {
            use prometheus::{Encoder, TextEncoder};
            use tokio::io::AsyncWriteExt;

            let encoder = TextEncoder::new();
            let metric_families = registry.gather();
            let mut buffer = Vec::new();
            if let Err(e) = encoder.encode(&metric_families, &mut buffer) {
                tracing::warn!("Failed to encode metrics: {}", e);
                return;
            }

            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/plain; version=0.0.4\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                buffer.len(),
                String::from_utf8_lossy(&buffer)
            );

            let _ = stream.write_all(response.as_bytes()).await;
            let _ = stream.flush().await;
        });
    }
}

fn error_response(status: StatusCode, message: &'static str) -> Response<ProxyBody> {
    Response::builder()
        .status(status)
        .body(
            Full::new(Bytes::from_static(message.as_bytes()))
                .map_err(|never| match never {})
                .boxed(),
        )
        .expect("static error response is valid")
}

async fn handle_request(
    req: Request<hyper::body::Incoming>,
    client_addr: SocketAddr,
    client_proto: &'static str,
    config_handle: Arc<RwLock<config::Config>>,
    metrics: Arc<metrics::Metrics>,
    lb_cache: LbCache,
) -> Result<Response<ProxyBody>, hyper::Error> {
    let start = Instant::now();
    metrics.http_requests_total.inc();

    // Request routing is not implemented yet: all traffic goes to the first
    // route. Read from the live config handle so SIGHUP reloads apply.
    let (upstream_addr, total_timeout) = {
        let config = config_handle.read().await;
        let route = match config.route.first() {
            Some(route) => route,
            None => {
                tracing::warn!("No routes configured");
                return Ok(error_response(
                    StatusCode::BAD_GATEWAY,
                    "Bad Gateway: no routes configured",
                ));
            }
        };

        let lb = {
            let cache = lb_cache.read().await;
            cache.get(&route.name).cloned()
        };
        let upstream_addr = match lb.as_ref().and_then(|lb| lb.next()) {
            Some(upstream) => upstream.addr.clone(),
            None => {
                tracing::warn!(route = %route.name, "No upstreams available");
                return Ok(error_response(
                    StatusCode::BAD_GATEWAY,
                    "Bad Gateway: no upstreams available",
                ));
            }
        };
        let total_timeout = route
            .timeout
            .total_duration()
            .unwrap_or(DEFAULT_TOTAL_TIMEOUT);
        (upstream_addr, total_timeout)
    };

    tracing::debug!(method = %req.method(), uri = %req.uri(), upstream = %upstream_addr, "Proxying request");

    // The timeout covers connecting and waiting for upstream response
    // headers; the body then streams through without buffering.
    let result = tokio::time::timeout(
        total_timeout,
        proxy::proxy_request(req, &upstream_addr, client_addr, client_proto),
    )
    .await;

    match result {
        Ok(Ok(response)) => {
            metrics
                .http_request_duration_seconds
                .observe(start.elapsed().as_secs_f64());
            Ok(response.map(|body| body.boxed()))
        }
        Ok(Err(e)) => {
            tracing::error!(upstream = %upstream_addr, "Proxy request failed: {}", e);
            metrics.upstream_connection_errors.inc();
            Ok(error_response(StatusCode::BAD_GATEWAY, "Bad Gateway"))
        }
        Err(_) => {
            tracing::error!(upstream = %upstream_addr, timeout_ms = %total_timeout.as_millis(), "Upstream timed out");
            metrics.upstream_connection_errors.inc();
            Ok(error_response(
                StatusCode::GATEWAY_TIMEOUT,
                "Gateway Timeout",
            ))
        }
    }
}
