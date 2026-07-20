use hyper::header::{HeaderMap, HeaderName, HeaderValue};
use hyper::{Request, Response};
use std::net::SocketAddr;

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

pub async fn proxy_request(
    mut req: Request<hyper::body::Incoming>,
    upstream: &str,
    client_addr: SocketAddr,
    client_proto: &'static str,
) -> anyhow::Result<Response<hyper::body::Incoming>> {
    // For HTTP/1.1 upstream requests the request line uses a relative URI.
    let path_and_query = req
        .uri()
        .path_and_query()
        .map(|pq| pq.as_str())
        .unwrap_or("/")
        .to_string();
    *req.uri_mut() = path_and_query.parse::<hyper::Uri>()?;

    let (host, port) = upstream_host_port(upstream)?;

    strip_hop_by_hop(req.headers_mut());
    req.headers_mut().insert(
        hyper::header::HOST,
        HeaderValue::from_str(&format!("{}:{}", host, port))?,
    );
    append_forwarded_headers(req.headers_mut(), client_addr, client_proto);

    let stream = tokio::net::TcpStream::connect((host.as_str(), port)).await?;
    let io = hyper_util::rt::TokioIo::new(stream);

    let (mut sender, conn) = hyper::client::conn::http1::Builder::new()
        .handshake(io)
        .await?;

    tokio::spawn(async move {
        if let Err(e) = conn.await {
            tracing::debug!("Upstream connection closed with error: {}", e);
        }
    });

    let mut response = sender.send_request(req).await?;
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
