use prometheus::{Histogram, HistogramOpts, IntCounter, Registry};

#[derive(Clone)]
pub struct Metrics {
    pub http_requests_total: IntCounter,
    pub http_request_duration_seconds: Histogram,
    pub upstream_connection_errors: IntCounter,
}

pub fn init_metrics() -> anyhow::Result<(Metrics, Registry)> {
    let registry = Registry::new();

    let http_requests_total = IntCounter::new("http_requests_total", "Total HTTP requests")?;
    registry.register(Box::new(http_requests_total.clone()))?;

    let http_request_duration_seconds = Histogram::with_opts(
        HistogramOpts::new(
            "http_request_duration_seconds",
            "Time from receiving a request to receiving upstream response headers",
        )
        .buckets(vec![
            0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0,
        ]),
    )?;
    registry.register(Box::new(http_request_duration_seconds.clone()))?;

    let upstream_connection_errors =
        IntCounter::new("upstream_connection_errors", "Upstream connection errors")?;
    registry.register(Box::new(upstream_connection_errors.clone()))?;

    Ok((
        Metrics {
            http_requests_total,
            http_request_duration_seconds,
            upstream_connection_errors,
        },
        registry,
    ))
}
