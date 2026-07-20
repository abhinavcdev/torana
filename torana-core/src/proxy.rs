use bytes::Bytes;
use http_body_util::combinators::BoxBody;
use http_body_util::{BodyExt, Empty};
use hyper::header::{HeaderMap, HeaderName, HeaderValue};
use hyper::{Request, Response};
use hyper_util::client::legacy::{connect::HttpConnector, Client};
use hyper_util::rt::TokioExecutor;
use std::net::SocketAddr;
use std::time::Duration;

/// Headers that must not be forwarded by a proxy (RFC 9110 §7.6.1).
const HOP_BY_HOP_HEADERS: [&str; 8] = [
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
];

/// Request body type sent to upstreams. Boxed so both the original
/// streaming client body and a freshly-built empty body (used to retry
/// GET/HEAD/OPTIONS requests on a different upstream) share one type.
pub type UpstreamBody = BoxBody<Bytes, hyper::Error>;

/// Shared upstream client with a keep-alive connection pool (keyed by
/// host:port). Cloning is cheap; all clones share one pool.
pub type UpstreamClient = Client<HttpConnector, UpstreamBody>;

pub fn build_upstream_client() -> UpstreamClient {
    let mut connector = HttpConnector::new();
    // Nagle's algorithm interacts badly with delayed ACKs and shows up
    // directly in tail latency; disable it on upstream connections.
    connector.set_nodelay(true);
    Client::builder(TokioExecutor::new())
        .pool_idle_timeout(Duration::from_secs(90))
        .pool_max_idle_per_host(128)
        .build(connector)
}

/// An empty request body, cheap to construct fresh for every retry attempt.
pub fn empty_body() -> UpstreamBody {
    Empty::<Bytes>::new()
        .map_err(|never| match never {})
        .boxed()
}

/// True if `err` (as returned by `proxy_request`) failed before any bytes
/// reached the upstream — i.e. it is safe to retry on a different upstream
/// without risking a non-idempotent request being applied twice.
pub fn is_retryable(err: &anyhow::Error) -> bool {
    err.downcast_ref::<hyper_util::client::legacy::Error>()
        .map(|e| e.is_connect())
        .unwrap_or(false)
}

pub async fn proxy_request(
    mut req: Request<UpstreamBody>,
    upstream: &str,
    client_addr: SocketAddr,
    client_proto: &'static str,
    client: &UpstreamClient,
) -> anyhow::Result<Response<hyper::body::Incoming>> {
    let path_and_query = req
        .uri()
        .path_and_query()
        .map(|pq| pq.as_str())
        .unwrap_or("/");
    let (host, port) = upstream_host_port(upstream)?;

    // The pooled client routes by absolute URI authority.
    let uri = format!("http://{}:{}{}", host, port, path_and_query).parse::<hyper::Uri>()?;
    *req.uri_mut() = uri;

    strip_hop_by_hop(req.headers_mut());
    req.headers_mut().insert(
        hyper::header::HOST,
        HeaderValue::from_str(&format!("{}:{}", host, port))?,
    );
    append_forwarded_headers(req.headers_mut(), client_addr, client_proto);

    let mut response = client.request(req).await?;
    strip_hop_by_hop(response.headers_mut());

    tracing::debug!(status = %response.status(), upstream, "Upstream responded");

    Ok(response)
}

fn strip_hop_by_hop(headers: &mut HeaderMap) {
    // The Connection header may name additional hop-by-hop headers.
    let named: Vec<HeaderName> = headers
        .get_all(hyper::header::CONNECTION)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .filter_map(|name| name.trim().parse::<HeaderName>().ok())
        .collect();
    for name in named {
        headers.remove(name);
    }
    for name in HOP_BY_HOP_HEADERS {
        headers.remove(name);
    }
}

fn append_forwarded_headers(headers: &mut HeaderMap, client_addr: SocketAddr, proto: &'static str) {
    let client_ip = client_addr.ip().to_string();
    let xff = match headers.get("x-forwarded-for").and_then(|v| v.to_str().ok()) {
        Some(existing) => format!("{}, {}", existing, client_ip),
        None => client_ip,
    };
    if let Ok(value) = HeaderValue::from_str(&xff) {
        headers.insert("x-forwarded-for", value);
    }
    headers.insert("x-forwarded-proto", HeaderValue::from_static(proto));
}

/// Split an upstream address like "http://host:port" into host and port.
pub fn upstream_host_port(upstream: &str) -> anyhow::Result<(String, u16)> {
    let rest = upstream
        .strip_prefix("http://")
        .unwrap_or(upstream)
        .split('/')
        .next()
        .unwrap_or("");
    let (host, port) = match rest.rsplit_once(':') {
        Some((host, port)) => (host, port.parse::<u16>()?),
        None => (rest, 80),
    };
    if host.is_empty() {
        anyhow::bail!("Could not extract host from '{}'", upstream);
    }
    Ok((host.to_string(), port))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_port_with_scheme_and_port() {
        assert_eq!(
            upstream_host_port("http://localhost:9999").unwrap(),
            ("localhost".to_string(), 9999)
        );
    }

    #[test]
    fn host_port_defaults_to_80() {
        assert_eq!(
            upstream_host_port("http://example.com").unwrap(),
            ("example.com".to_string(), 80)
        );
    }

    #[test]
    fn host_port_without_scheme() {
        assert_eq!(
            upstream_host_port("127.0.0.1:8080").unwrap(),
            ("127.0.0.1".to_string(), 8080)
        );
    }

    #[test]
    fn host_port_ignores_path() {
        assert_eq!(
            upstream_host_port("http://example.com:8080/path").unwrap(),
            ("example.com".to_string(), 8080)
        );
    }

    #[test]
    fn host_port_rejects_empty() {
        assert!(upstream_host_port("http://").is_err());
        assert!(upstream_host_port("http://:8080").is_err());
    }

    #[test]
    fn strips_hop_by_hop_headers() {
        let mut headers = HeaderMap::new();
        headers.insert("connection", HeaderValue::from_static("close, x-custom"));
        headers.insert("keep-alive", HeaderValue::from_static("timeout=5"));
        headers.insert("x-custom", HeaderValue::from_static("1"));
        headers.insert("x-kept", HeaderValue::from_static("1"));
        strip_hop_by_hop(&mut headers);
        assert!(headers.get("connection").is_none());
        assert!(headers.get("keep-alive").is_none());
        assert!(headers.get("x-custom").is_none());
        assert!(headers.get("x-kept").is_some());
    }

    #[test]
    fn appends_to_existing_xff() {
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", HeaderValue::from_static("10.0.0.1"));
        let addr: SocketAddr = "192.168.1.5:1234".parse().unwrap();
        append_forwarded_headers(&mut headers, addr, "https");
        assert_eq!(headers["x-forwarded-for"], "10.0.0.1, 192.168.1.5");
        assert_eq!(headers["x-forwarded-proto"], "https");
    }
}
