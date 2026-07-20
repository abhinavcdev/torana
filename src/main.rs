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
use tokio::sync::{watch, RwLock};

/// Applied when a route does not set timeout.total.
const DEFAULT_TOTAL_TIMEOUT: Duration = Duration::from_secs(30);

/// How long shutdown waits for in-flight connections to finish.
const DRAIN_TIMEOUT: Duration = Duration::from_secs(15);

type ProxyBody = BoxBody<Bytes, hyper::Error>;
type LbCache = Arc<RwLock<HashMap<String, Arc<upstream::LoadBalancer>>>>;

#[derive(Parser)]
#[command(name = "torana")]
#[command(about = "Rust-native micro reverse proxy")]
struct Cli {
    /// Path to config file
    #[arg(short, long, default_value = "torana.toml")]
    config: String,
}

/// Everything a connection needs, cloned per accept.
#[derive(Clone)]
struct Handler {
    config_handle: Arc<RwLock<config::Config>>,
    metrics: Arc<metrics::Metrics>,
    lb_cache: LbCache,
    upstream_client: proxy::UpstreamClient,
    shutdown_rx: watch::Receiver<bool>,
    /// Each live connection holds a clone; shutdown waits for the strong
    /// count to fall back to the listeners' baseline.
    conn_tracker: Arc<()>,
}

async fn shutdown_signal() {
    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .expect("Failed to install SIGTERM handler");
    tokio::select! {
        _ = sigterm.recv() => tracing::info!("SIGTERM received, draining connections"),
        _ = tokio::signal::ctrl_c() => tracing::info!("SIGINT received, draining connections"),
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let config = load_config(&cli.config)?;

    let log_level = config.global.log_level.as_deref().unwrap_or("info");
    let log_format = config.global.log_format.as_deref().unwrap_or("json");
    logging::init_logging(log_format, log_level)?;

    config.validate()?;

    tracing::info!("torana starting");

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let (metrics, registry) = init_metrics()?;
    let handler = Handler {
        config_handle: Arc::new(RwLock::new(config.clone())),
        metrics: Arc::new(metrics),
        lb_cache: Arc::new(RwLock::new(upstream::build_lb_map(&config))),
        upstream_client: proxy::build_upstream_client(),
        shutdown_rx,
        conn_tracker: Arc::new(()),
    };

    // SIGHUP triggers a config reload that also rebuilds the load balancers.
    tokio::spawn(reload::watch_config_signal(
        cli.config.clone(),
        handler.config_handle.clone(),
        handler.lb_cache.clone(),
    ));

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

        if listener_cfg.protocol == "https" {
            let cert_path = listener_cfg.tls_cert.clone().expect("validated");
            let key_path = listener_cfg.tls_key.clone().expect("validated");
            let acceptor = tls::load_tls_config(&cert_path, &key_path)?;
            tracing::info!("HTTPS listener started on {}", addr);
            tokio::spawn(run_listener(tcp, Some(acceptor), handler.clone()));
        } else {
            tracing::info!("HTTP listener started on {}", addr);
            tokio::spawn(run_listener(tcp, None, handler.clone()));
        }
    }

    tracing::info!("torana ready");

    shutdown_signal().await;

    // Stop accepting, ask in-flight connections to finish, then wait.
    let _ = shutdown_tx.send(true);
    let baseline = 1; // main's own handler clone is dropped below
    let tracker = handler.conn_tracker.clone();
    drop(handler);
    let deadline = Instant::now() + DRAIN_TIMEOUT;
    while Arc::strong_count(&tracker) > baseline && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let leftover = Arc::strong_count(&tracker) - baseline;
    if leftover > 0 {
        tracing::warn!("Drain timeout: {} connections still open", leftover);
    }
    tracing::info!("Shutdown complete");
    Ok(())
}

async fn accept_or_retry(
    listener: &tokio::net::TcpListener,
) -> (tokio::net::TcpStream, SocketAddr) {
    loop {
        match listener.accept().await {
            Ok((stream, peer)) => {
                // Nagle + delayed ACK adds tens of ms to tail latency.
                let _ = stream.set_nodelay(true);
                return (stream, peer);
            }
            Err(e) => {
                // Transient errors (EMFILE, ECONNABORTED) must not kill the
                // accept loop.
                tracing::warn!("Accept error: {}", e);
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }
    }
}

/// Accept loop for one listener; `acceptor` is Some for HTTPS. Stops
/// accepting when shutdown is signalled; live connections drain gracefully.
async fn run_listener(
    listener: tokio::net::TcpListener,
    acceptor: Option<tokio_rustls::TlsAcceptor>,
    handler: Handler,
) {
    let mut accept_shutdown = handler.shutdown_rx.clone();
    loop {
        let (stream, peer) = tokio::select! {
            conn = accept_or_retry(&listener) => conn,
            _ = accept_shutdown.changed() => break,
        };
        let acceptor = acceptor.clone();
        let handler = handler.clone();

        tokio::spawn(async move {
            match acceptor {
                Some(acceptor) => match acceptor.accept(stream).await {
                    Ok(tls_stream) => {
                        serve_connection(
                            hyper_util::rt::TokioIo::new(tls_stream),
                            peer,
                            "https",
                            handler,
                        )
                        .await
                    }
                    Err(e) => tracing::debug!("TLS handshake failed: {}", e),
                },
                None => {
                    serve_connection(hyper_util::rt::TokioIo::new(stream), peer, "http", handler)
                        .await
                }
            }
        });
    }
}

/// Serve one client connection, finishing the in-flight response before
/// closing if shutdown is signalled mid-request.
async fn serve_connection<I>(io: I, peer: SocketAddr, proto: &'static str, handler: Handler)
where
    I: hyper::rt::Read + hyper::rt::Write + Unpin + Send + 'static,
{
    let _guard = handler.conn_tracker.clone();
    let mut conn_shutdown = handler.shutdown_rx.clone();

    let service = service_fn(move |req: Request<hyper::body::Incoming>| {
        let handler = handler.clone();
        handle_request(req, peer, proto, handler)
    });

    let conn = hyper::server::conn::http1::Builder::new().serve_connection(io, service);
    let mut conn = std::pin::pin!(conn);

    tokio::select! {
        result = conn.as_mut() => {
            if let Err(e) = result {
                tracing::debug!("Connection error: {}", e);
            }
        }
        _ = conn_shutdown.changed() => {
            // Finish the current response, then close.
            conn.as_mut().graceful_shutdown();
            if let Err(e) = conn.as_mut().await {
                tracing::debug!("Connection error during drain: {}", e);
            }
        }
    }
}

/// Minimal HTTP server for /metrics (Prometheus text format) and /healthz.
async fn serve_metrics(listener: tokio::net::TcpListener, registry: prometheus::Registry) {
    loop {
        let (mut stream, _) = accept_or_retry(&listener).await;
        let registry = registry.clone();

        tokio::spawn(async move {
            use prometheus::{Encoder, TextEncoder};
            use tokio::io::{AsyncReadExt, AsyncWriteExt};

            // Read the request line to route; ignore the rest of the head.
            let mut buf = [0u8; 1024];
            let n = match tokio::time::timeout(Duration::from_secs(2), stream.read(&mut buf)).await
            {
                Ok(Ok(n)) if n > 0 => n,
                _ => return,
            };
            let head = String::from_utf8_lossy(&buf[..n]);
            let path = head
                .lines()
                .next()
                .and_then(|line| line.split_whitespace().nth(1))
                .unwrap_or("/");

            let (status, content_type, body) = if path.starts_with("/healthz") {
                ("200 OK", "text/plain", "ok".to_string())
            } else {
                let encoder = TextEncoder::new();
                let mut buffer = Vec::new();
                if let Err(e) = encoder.encode(&registry.gather(), &mut buffer) {
                    tracing::warn!("Failed to encode metrics: {}", e);
                    return;
                }
                (
                    "200 OK",
                    "text/plain; version=0.0.4",
                    String::from_utf8_lossy(&buffer).into_owned(),
                )
            };

            let response = format!(
                "HTTP/1.1 {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                status,
                content_type,
                body.len(),
                body
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
    handler: Handler,
) -> Result<Response<ProxyBody>, hyper::Error> {
    let start = Instant::now();
    handler.metrics.http_requests_total.inc();

    // Request routing is not implemented yet: all traffic goes to the first
    // route. Read from the live config handle so SIGHUP reloads apply.
    let (upstream_addr, total_timeout) = {
        let config = handler.config_handle.read().await;
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
            let cache = handler.lb_cache.read().await;
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
        proxy::proxy_request(
            req,
            &upstream_addr,
            client_addr,
            client_proto,
            &handler.upstream_client,
        ),
    )
    .await;

    match result {
        Ok(Ok(response)) => {
            handler
                .metrics
                .http_request_duration_seconds
                .observe(start.elapsed().as_secs_f64());
            Ok(response.map(|body| body.boxed()))
        }
        Ok(Err(e)) => {
            tracing::error!(upstream = %upstream_addr, "Proxy request failed: {}", e);
            handler.metrics.upstream_connection_errors.inc();
            Ok(error_response(StatusCode::BAD_GATEWAY, "Bad Gateway"))
        }
        Err(_) => {
            tracing::error!(upstream = %upstream_addr, timeout_ms = %total_timeout.as_millis(), "Upstream timed out");
            handler.metrics.upstream_connection_errors.inc();
            Ok(error_response(
                StatusCode::GATEWAY_TIMEOUT,
                "Gateway Timeout",
            ))
        }
    }
}
