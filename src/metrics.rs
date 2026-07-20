use prometheus::{IntCounter, Histogram, HistogramOpts, Registry};

#[derive(Clone)]
pub struct Metrics {
    pub http_requests_total: IntCounter,
    pub http_request_duration_ms: Histogram,
    pub http_request_size_bytes: Histogram,
    pub upstream_connection_errors: IntCounter,
}

pub fn init_metrics() -> anyhow::Result<(Metrics, Registry)> {
    let registry = Registry::new();

    let http_requests_total = IntCounter::new(
        "http_requests_total",
        "Total HTTP requests",
    )?;
    registry.register(Box::new(http_requests_total.clone()))?;

    let http_request_duration_ms = Histogram::with_opts(
        HistogramOpts::new(
            "http_request_duration_ms",
            "HTTP request latency",
        ),
    )?;
    registry.register(Box::new(http_request_duration_ms.clone()))?;

    let http_request_size_bytes = Histogram::with_opts(
        HistogramOpts::new(
            "http_request_size_bytes",
            "HTTP request size",
        ),
    )?;
    registry.register(Box::new(http_request_size_bytes.clone()))?;

    let upstream_connection_errors = IntCounter::new(
        "upstream_connection_errors",
        "Upstream connection errors",
    )?;
    registry.register(Box::new(upstream_connection_errors.clone()))?;

    Ok((
        Metrics {
            http_requests_total,
            http_request_duration_ms,
            http_request_size_bytes,
            upstream_connection_errors,
        },
        registry,
    ))
}
