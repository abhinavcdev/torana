use crate::config::Config;
use crate::health::HealthRegistry;
use crate::metrics::{init_metrics, Metrics};
use crate::reload::{self, ReloadTargets};
use crate::routing;
use crate::upstream::{self, LoadBalancer};
use crate::{logging, proxy, tls};
use bytes::Bytes;
use http_body_util::combinators::BoxBody;
use http_body_util::{BodyExt, Full};
use hyper::body::Body as _;
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{watch, Mutex, RwLock};

/// Applied when a route does not set timeout.total.
const DEFAULT_TOTAL_TIMEOUT: Duration = Duration::from_secs(30);
/// How long graceful shutdown waits for in-flight connections to finish.
const DRAIN_TIMEOUT: Duration = Duration::from_secs(15);

pub type ProxyBody = BoxBody<Bytes, hyper::Error>;
type LbCache = Arc<RwLock<HashMap<String, Arc<LoadBalancer>>>>;
#[cfg(feature = "plugins")]
type PluginCache = Arc<RwLock<HashMap<String, Arc<crate::plugin::Plugin>>>>;

/// The routing-and-forwarding core, with none of the standalone process's
/// listener/signal/metrics machinery. Embed this inside a hyper server you
/// already run: construct one, then call [`ProxyEngine::handle`] from your
/// own `service_fn`. [`Server`] is a thin wrapper around this for the
/// batteries-included standalone binary.
#[derive(Clone)]
pub struct ProxyEngine {
    config_handle: Arc<RwLock<Config>>,
    lb_cache: LbCache,
    health_registry: HealthRegistry,
    #[cfg(feature = "plugins")]
    plugin_cache: PluginCache,
    upstream_client: proxy::UpstreamClient,
    metrics: Arc<Metrics>,
}

impl ProxyEngine {
    pub fn new(config: Config, metrics: Arc<Metrics>) -> Self {
        #[cfg(feature = "plugins")]
        let plugin_cache = match crate::plugin::build_plugin_cache(&config) {
            Ok(cache) => cache,
            Err(e) => {
                tracing::error!("failed to load plugins at startup: {:#}", e);
                HashMap::new()
            }
        };

        ProxyEngine {
            lb_cache: Arc::new(RwLock::new(upstream::build_lb_map(&config))),
            health_registry: HealthRegistry::new(),
            #[cfg(feature = "plugins")]
            plugin_cache: Arc::new(RwLock::new(plugin_cache)),
            upstream_client: proxy::build_upstream_client(),
            config_handle: Arc::new(RwLock::new(config)),
            metrics,
        }
    }

    /// Spawn active health-check probers for every route with
    /// `health_check` configured. Embedders that want health checking must
    /// call this once after construction; [`Server`] does it automatically.
    pub async fn spawn_health_probers(&self) -> Vec<tokio::task::JoinHandle<()>> {
        let config = self.config_handle.read().await;
        crate::health::spawn_all_probers(&config, self.health_registry.clone())
    }

    fn reload_targets(
        &self,
        health_handles: Arc<Mutex<Vec<tokio::task::JoinHandle<()>>>>,
    ) -> ReloadTargets {
        ReloadTargets {
            config_handle: self.config_handle.clone(),
            lb_cache: self.lb_cache.clone(),
            health_registry: self.health_registry.clone(),
            health_handles,
            #[cfg(feature = "plugins")]
            plugin_cache: self.plugin_cache.clone(),
        }
    }

    /// Route, load-balance, retry, and forward one request. Never buffers
    /// request or response bodies (except for the empty body used to retry
    /// idempotent requests). Timeouts and connection errors become 504/502
    /// responses rather than propagated errors.
    ///
    /// `client_cert_fingerprint` is `Some` only when the connection
    /// completed an mTLS handshake against a listener's `tls_client_ca`;
    /// pass `None` for plain HTTP/TLS connections.
    pub async fn handle(
        &self,
        req: Request<hyper::body::Incoming>,
        client_addr: SocketAddr,
        client_proto: &'static str,
        client_cert_fingerprint: Option<String>,
    ) -> Result<Response<ProxyBody>, hyper::Error> {
        let start = Instant::now();
        self.metrics.http_requests_total.inc();

        let host_header = req
            .headers()
            .get(hyper::header::HOST)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let path = req.uri().path().to_string();
        let method = req.method().clone();

        let route_name;
        let max_attempts;
        let total_timeout;
        let request_headers;
        let response_headers;
        let mirror;
        #[cfg(feature = "plugins")]
        let plugin: Option<Arc<crate::plugin::Plugin>>;
        {
            let config = self.config_handle.read().await;
            let route = match routing::select_route(&config.route, host_header.as_deref(), &path) {
                Some(route) => route,
                None => {
                    tracing::warn!(path = %path, host = ?host_header, "No route matched request");
                    return Ok(error_response(
                        StatusCode::NOT_FOUND,
                        "Not Found: no route matched",
                    ));
                }
            };
            route_name = route.name.clone();
            max_attempts = route.max_attempts();
            total_timeout = route
                .timeout
                .total_duration()
                .unwrap_or(DEFAULT_TOTAL_TIMEOUT);
            request_headers = route.headers.request.clone();
            response_headers = route.headers.response.clone();
            mirror = route.mirror.clone();
            #[cfg(feature = "plugins")]
            {
                plugin = match &route.plugin {
                    Some(path) => self.plugin_cache.read().await.get(path).cloned(),
                    None => None,
                };
            }
        }

        #[cfg(feature = "plugins")]
        if let Some(plugin) = &plugin {
            match plugin.evaluate(method.as_str(), &path) {
                crate::plugin::Verdict::Allow => {}
                crate::plugin::Verdict::Deny(status) => {
                    let code = StatusCode::from_u16(status).unwrap_or(StatusCode::FORBIDDEN);
                    tracing::info!(route = %route_name, status = %code, "Request denied by plugin");
                    return Ok(error_response(code, "Denied by plugin"));
                }
            }
        }

        // Retries and mirroring both need to reconstruct the request from
        // scratch, which requires a body we can cheaply recreate — safe
        // only when the method carries no meaningful body and the client
        // sent none. When that doesn't hold, effective_attempts is forced
        // to 1, so the streaming body below is taken exactly once, and
        // mirroring is skipped for this request.
        let (parts, incoming_body) = req.into_parts();
        let body_replayable = matches!(parts.method, Method::GET | Method::HEAD | Method::OPTIONS)
            && incoming_body.size_hint().exact() == Some(0);
        let can_retry = max_attempts > 1 && body_replayable;
        let effective_attempts = if can_retry { max_attempts } else { 1 };
        let mut incoming_body = Some(incoming_body);

        if let Some(mirror) = &mirror {
            if body_replayable && should_mirror(mirror.rate.unwrap_or(100)) {
                let mirror_req = build_request(&parts, proxy::empty_body());
                let mirror_addr = mirror.addr.clone();
                let mirror_client = self.upstream_client.clone();
                let mirror_timeout = total_timeout;
                tokio::spawn(async move {
                    let result = tokio::time::timeout(
                        mirror_timeout,
                        proxy::proxy_request(
                            mirror_req,
                            &mirror_addr,
                            client_addr,
                            client_proto,
                            &mirror_client,
                        ),
                    )
                    .await;
                    match result {
                        Ok(Ok(_)) => {}
                        Ok(Err(e)) => {
                            tracing::debug!(mirror = %mirror_addr, "mirror request failed: {}", e)
                        }
                        Err(_) => {
                            tracing::debug!(mirror = %mirror_addr, "mirror request timed out")
                        }
                    }
                });
            } else if !body_replayable {
                tracing::debug!(route = %route_name, "skipping mirror: request body cannot be safely duplicated");
            }
        }

        let mut tried: Vec<String> = Vec::new();
        let mut last_error: Option<anyhow::Error> = None;

        for attempt in 0..effective_attempts {
            let upstream_addr = {
                let cache = self.lb_cache.read().await;
                let lb = cache.get(&route_name).cloned();
                drop(cache);
                match lb {
                    Some(lb) => lb
                        .next_healthy(&self.health_registry, &tried)
                        .await
                        .map(|u| u.addr.clone()),
                    None => None,
                }
            };
            let Some(upstream_addr) = upstream_addr else {
                tracing::warn!(route = %route_name, "No upstreams available");
                return Ok(error_response(
                    StatusCode::BAD_GATEWAY,
                    "Bad Gateway: no upstreams available",
                ));
            };

            let mut attempt_req = if can_retry {
                build_request(&parts, proxy::empty_body())
            } else {
                let body = incoming_body
                    .take()
                    .expect("non-retry path runs exactly one attempt");
                build_request(&parts, body.boxed())
            };
            if let Some(overrides) = &request_headers {
                proxy::apply_header_overrides(attempt_req.headers_mut(), overrides);
            }
            // Client identity from a verified mTLS handshake is
            // security-relevant: always strip whatever a client sent for
            // this header, then set it ourselves only if this connection
            // actually completed mTLS, so it can never be spoofed by a
            // client on a plain HTTP/TLS listener or route override above.
            attempt_req
                .headers_mut()
                .remove("x-client-cert-fingerprint");
            if let Some(fingerprint) = &client_cert_fingerprint {
                attempt_req.headers_mut().insert(
                    hyper::header::HeaderName::from_static("x-client-cert-fingerprint"),
                    hyper::header::HeaderValue::from_str(fingerprint)
                        .expect("hex-encoded fingerprint is always a valid header value"),
                );
            }

            tracing::debug!(method = %method, uri = %path, upstream = %upstream_addr, attempt, "Proxying request");

            let result = tokio::time::timeout(
                total_timeout,
                proxy::proxy_request(
                    attempt_req,
                    &upstream_addr,
                    client_addr,
                    client_proto,
                    &self.upstream_client,
                ),
            )
            .await;

            match result {
                Ok(Ok(mut response)) => {
                    self.metrics
                        .http_request_duration_seconds
                        .observe(start.elapsed().as_secs_f64());
                    if let Some(overrides) = &response_headers {
                        proxy::apply_header_overrides(response.headers_mut(), overrides);
                    }
                    return Ok(response.map(|body| body.boxed()));
                }
                Ok(Err(e)) => {
                    self.metrics.upstream_connection_errors.inc();
                    let retryable = proxy::is_retryable(&e);
                    tracing::warn!(upstream = %upstream_addr, attempt, retryable, "Proxy request failed: {}", e);
                    tried.push(upstream_addr);
                    last_error = Some(e);
                    if !can_retry || !retryable {
                        break;
                    }
                }
                Err(_) => {
                    self.metrics.upstream_connection_errors.inc();
                    tracing::error!(upstream = %upstream_addr, timeout_ms = %total_timeout.as_millis(), "Upstream timed out");
                    return Ok(error_response(
                        StatusCode::GATEWAY_TIMEOUT,
                        "Gateway Timeout",
                    ));
                }
            }
        }

        tracing::error!(route = %route_name, attempts = tried.len(), "All upstream attempts failed: {:?}", last_error);
        Ok(error_response(StatusCode::BAD_GATEWAY, "Bad Gateway"))
    }
}

/// Build a fresh request from a request's head (method/uri/version/headers)
/// and a body. `http::request::Parts` isn't `Clone` (it carries type-erased
/// `Extensions`), so each retry attempt clones just the fields that matter.
fn build_request(
    parts: &hyper::http::request::Parts,
    body: proxy::UpstreamBody,
) -> Request<proxy::UpstreamBody> {
    // The upstream leg always speaks HTTP/1.1, regardless of what the
    // client negotiated (h2 clients are common; h2 upstreams are not, and
    // this proxy doesn't support them). Copying the client's HTTP/2
    // version through would make hyper-util's http1-only upstream client
    // correctly refuse to send it (`UserUnsupportedVersion`).
    let mut builder = Request::builder()
        .method(parts.method.clone())
        .uri(parts.uri.clone())
        .version(hyper::Version::HTTP_11);
    if let Some(headers) = builder.headers_mut() {
        *headers = parts.headers.clone();
    }
    builder
        .body(body)
        .expect("rebuilding a request from a previously-valid head cannot fail")
}

/// Approximate sampling for `mirror.rate` (0-100). Uses the current
/// timestamp's sub-second nanoseconds as a cheap, allocation-free source of
/// variation — good enough for traffic sampling, not a cryptographic RNG,
/// and avoids adding a `rand` dependency or extra shared per-route state
/// that reload would need to keep in sync with the load balancer.
fn should_mirror(rate: u32) -> bool {
    if rate >= 100 {
        return true;
    }
    if rate == 0 {
        return false;
    }
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    (nanos % 100) < rate
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

/// Everything a live connection needs, cloned per accept.
#[derive(Clone)]
struct ConnHandler {
    engine: ProxyEngine,
    shutdown_rx: watch::Receiver<bool>,
    /// Each live connection holds a clone; shutdown waits for the strong
    /// count to fall back to the listeners' baseline.
    conn_tracker: Arc<()>,
}

/// The full standalone process: binds every configured listener, runs the
/// metrics/healthz endpoint, reloads on SIGHUP, and drains connections
/// gracefully on SIGTERM/SIGINT.
pub struct Server {
    config: Config,
}

impl Server {
    pub fn new(config: Config) -> Self {
        Server { config }
    }

    /// Run until SIGTERM/SIGINT, then drain and return. `config_path` is
    /// re-read on every SIGHUP.
    pub async fn run(self, config_path: String) -> anyhow::Result<()> {
        let config = self.config;
        tracing::info!("torana starting");

        // rustls 0.23 refuses to auto-select a crypto provider when more
        // than one is compiled into the process (our own dependency pins
        // "ring" explicitly, but other crates in the graph can still pull
        // in "aws-lc-rs"). Install ours up front so a fresh ambiguity
        // introduced by some future dependency fails loudly in tests
        // rather than at a customer's first HTTPS request. Err just means
        // a provider was already installed, which is fine.
        let _ = rustls::crypto::ring::default_provider().install_default();

        let (metrics, registry) = init_metrics()?;
        let metrics = Arc::new(metrics);
        let engine = ProxyEngine::new(config.clone(), metrics.clone());

        let health_handles = Arc::new(Mutex::new(engine.spawn_health_probers().await));

        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let handler = ConnHandler {
            engine: engine.clone(),
            shutdown_rx,
            conn_tracker: Arc::new(()),
        };

        tokio::spawn(reload::watch_config_signal(
            config_path,
            engine.reload_targets(health_handles.clone()),
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

        for listener_cfg in &config.listener {
            let addr = listener_cfg.addr.parse::<SocketAddr>()?;
            let tcp = tokio::net::TcpListener::bind(addr)
                .await
                .map_err(|e| anyhow::anyhow!("Failed to bind {}: {}", addr, e))?;

            if listener_cfg.acme.is_some() {
                #[cfg(feature = "acme")]
                {
                    let acme_cfg = listener_cfg.acme.as_ref().expect("validated");
                    let handle = crate::acme::build(
                        acme_cfg.domains.clone(),
                        acme_cfg.contact_emails.clone().unwrap_or_default(),
                        acme_cfg.cache_dir().to_string(),
                        acme_cfg.directory_url.clone(),
                        acme_cfg.staging.unwrap_or(false),
                        acme_cfg.ca_cert.clone(),
                    )?;
                    tracing::info!(domains = ?acme_cfg.domains, "ACME-managed HTTPS listener started on {}", addr);
                    tokio::spawn(crate::acme::drive(handle.state));
                    tokio::spawn(run_acme_listener(
                        tcp,
                        handle.acceptor,
                        handle.rustls_config,
                        handler.clone(),
                    ));
                }
                #[cfg(not(feature = "acme"))]
                unreachable!("validated: acme requires --features acme");
            } else if listener_cfg.protocol == "https" {
                let cert_path = listener_cfg.tls_cert.clone().expect("validated");
                let key_path = listener_cfg.tls_key.clone().expect("validated");
                let acceptor = tls::load_tls_config(
                    &cert_path,
                    &key_path,
                    listener_cfg.tls_client_ca.as_deref(),
                )?;
                tracing::info!("HTTPS listener started on {}", addr);
                tokio::spawn(run_listener(tcp, Some(acceptor), handler.clone()));
            } else {
                tracing::info!("HTTP listener started on {}", addr);
                tokio::spawn(run_listener(tcp, None, handler.clone()));
            }
        }

        tracing::info!("torana ready");

        shutdown_signal().await;

        let _ = shutdown_tx.send(true);
        let baseline = 1; // this local `handler` clone, dropped next
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
}

/// Initializes global logging from config. Exposed here so `torana`'s thin
/// binary doesn't need its own dependency on `tracing-subscriber`.
pub fn init_logging_from_config(config: &Config) -> anyhow::Result<()> {
    let log_level = config.global.log_level.as_deref().unwrap_or("info");
    let log_format = config.global.log_format.as_deref().unwrap_or("json");
    logging::init_logging(log_format, log_level)
}

async fn shutdown_signal() {
    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .expect("Failed to install SIGTERM handler");
    tokio::select! {
        _ = sigterm.recv() => tracing::info!("SIGTERM received, draining connections"),
        _ = tokio::signal::ctrl_c() => tracing::info!("SIGINT received, draining connections"),
    }
}

async fn accept_or_retry(
    listener: &tokio::net::TcpListener,
) -> (tokio::net::TcpStream, SocketAddr) {
    loop {
        match listener.accept().await {
            Ok((stream, peer)) => {
                let _ = stream.set_nodelay(true);
                return (stream, peer);
            }
            Err(e) => {
                tracing::warn!("Accept error: {}", e);
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }
    }
}

async fn run_listener(
    listener: tokio::net::TcpListener,
    acceptor: Option<tokio_rustls::TlsAcceptor>,
    handler: ConnHandler,
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
                        // Present only when the listener requires client
                        // certificates (tls_client_ca) and the handshake
                        // verified one; one fingerprint per connection,
                        // reused across keep-alive requests.
                        let client_cert_fingerprint = tls_stream
                            .get_ref()
                            .1
                            .peer_certificates()
                            .and_then(|certs| certs.first())
                            .map(|cert| tls::cert_fingerprint(cert.as_ref()));
                        let use_h2 = tls_stream.get_ref().1.alpn_protocol() == Some(b"h2");
                        serve_connection(
                            hyper_util::rt::TokioIo::new(tls_stream),
                            peer,
                            "https",
                            handler,
                            client_cert_fingerprint,
                            use_h2,
                        )
                        .await
                    }
                    Err(e) => tracing::debug!("TLS handshake failed: {}", e),
                },
                None => {
                    // No TLS means no ALPN, so no negotiated h2; cleartext
                    // http/2 (h2c) is deliberately not supported.
                    serve_connection(
                        hyper_util::rt::TokioIo::new(stream),
                        peer,
                        "http",
                        handler,
                        None,
                        false,
                    )
                    .await
                }
            }
        });
    }
}

#[cfg(feature = "acme")]
async fn run_acme_listener(
    listener: tokio::net::TcpListener,
    acceptor: rustls_acme::AcmeAcceptor,
    rustls_config: Arc<rustls_acme::rustls::ServerConfig>,
    handler: ConnHandler,
) {
    use tokio_util::compat::TokioAsyncReadCompatExt;

    let mut accept_shutdown = handler.shutdown_rx.clone();
    loop {
        let (stream, peer) = tokio::select! {
            conn = accept_or_retry(&listener) => conn,
            _ = accept_shutdown.changed() => break,
        };
        let acceptor = acceptor.clone();
        let rustls_config = rustls_config.clone();
        let handler = handler.clone();

        tokio::spawn(async move {
            match crate::acme::accept(&acceptor, stream.compat(), rustls_config).await {
                Ok(Some(accepted)) => {
                    // ACME listeners don't support mTLS (validated at
                    // startup), so there is never a client cert fingerprint.
                    serve_connection(
                        hyper_util::rt::TokioIo::new(accepted.stream),
                        peer,
                        "https",
                        handler,
                        None,
                        accepted.use_h2,
                    )
                    .await
                }
                Ok(None) => {} // TLS-ALPN-01 validation traffic, handled internally
                Err(e) => tracing::debug!("ACME TLS handshake failed: {}", e),
            }
        });
    }
}

async fn serve_connection<I>(
    io: I,
    peer: SocketAddr,
    proto: &'static str,
    handler: ConnHandler,
    client_cert_fingerprint: Option<String>,
    use_h2: bool,
) where
    I: hyper::rt::Read + hyper::rt::Write + Unpin + Send + 'static,
{
    let _guard = handler.conn_tracker.clone();
    let mut conn_shutdown = handler.shutdown_rx.clone();

    let service = service_fn(move |req: Request<hyper::body::Incoming>| {
        let engine = handler.engine.clone();
        let client_cert_fingerprint = client_cert_fingerprint.clone();
        async move {
            engine
                .handle(req, peer, proto, client_cert_fingerprint)
                .await
        }
    });

    if use_h2 {
        let conn = hyper::server::conn::http2::Builder::new(hyper_util::rt::TokioExecutor::new())
            .serve_connection(io, service);
        let mut conn = std::pin::pin!(conn);
        tokio::select! {
            result = conn.as_mut() => {
                if let Err(e) = result {
                    tracing::debug!("h2 connection error: {}", e);
                }
            }
            _ = conn_shutdown.changed() => {
                conn.as_mut().graceful_shutdown();
                if let Err(e) = conn.as_mut().await {
                    tracing::debug!("h2 connection error during drain: {}", e);
                }
            }
        }
    } else {
        let conn = hyper::server::conn::http1::Builder::new().serve_connection(io, service);
        let mut conn = std::pin::pin!(conn);
        tokio::select! {
            result = conn.as_mut() => {
                if let Err(e) = result {
                    tracing::debug!("Connection error: {}", e);
                }
            }
            _ = conn_shutdown.changed() => {
                conn.as_mut().graceful_shutdown();
                if let Err(e) = conn.as_mut().await {
                    tracing::debug!("Connection error during drain: {}", e);
                }
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
