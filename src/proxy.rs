use hyper::{Request, Response};

pub async fn proxy_request(
    mut req: Request<hyper::body::Incoming>,
    upstream: &str,
) -> anyhow::Result<Response<hyper::body::Incoming>> {
    tracing::debug!("Proxying request to upstream: {}", upstream);

    // Build the request URI - for HTTP/1.1 requests, use relative URI (path and query)
    let path_and_query = req
        .uri()
        .path_and_query()
        .map(|pq| pq.as_str())
        .unwrap_or("/")
        .to_string();

    // Parse the path as a relative URI for the request line
    let new_uri = path_and_query.parse::<hyper::Uri>()?;
    *req.uri_mut() = new_uri;

    tracing::debug!("Forwarding with relative URI: {}", path_and_query);

    // Set the Host header to the upstream address
    let host_header = extract_host(upstream)?;
    req.headers_mut().insert(
        hyper::header::HOST,
        hyper::header::HeaderValue::from_str(&host_header)?
    );

    // Connect to upstream and forward request
    let host = extract_host(upstream)?;
    let port = extract_port(upstream)?;

    let stream = tokio::net::TcpStream::connect((host.as_str(), port)).await?;
    use hyper_util::rt::TokioIo;
    let io = TokioIo::new(stream);

    let (mut sender, conn) = hyper::client::conn::http1::Builder::new()
        .handshake(io)
        .await?;

    tokio::spawn(async move {
        if let Err(e) = conn.await {
            tracing::error!("Upstream connection error: {}", e);
        }
    });

    let response = sender.send_request(req).await?;

    tracing::debug!(
        "Received response from upstream with status: {}",
        response.status()
    );

    Ok(response)
}

fn extract_host(url: &str) -> anyhow::Result<String> {
    let url = url.trim_start_matches("http://").trim_start_matches("https://");
    let host = url.split('/').next().unwrap_or("").split(':').next().unwrap_or("");

    if host.is_empty() {
        anyhow::bail!("Could not extract host from URL");
    }

    Ok(host.to_string())
}

fn extract_port(url: &str) -> anyhow::Result<u16> {
    if url.starts_with("https://") {
        Ok(443)
    } else {
        let url = url.trim_start_matches("http://");
        let host_port = url.split('/').next().unwrap_or("");
        let port_str = host_port.split(':').nth(1).unwrap_or("80");
        Ok(port_str.parse()?)
    }
}
