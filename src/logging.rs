use tracing_subscriber::fmt;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;

pub fn init_logging(format: &str, level: &str) -> anyhow::Result<()> {
    let env_filter = EnvFilter::new(level);

    match format {
        "json" => {
            tracing_subscriber::registry()
                .with(env_filter)
                .with(fmt::layer().json())
                .init();
        }
        _ => {
            tracing_subscriber::registry()
                .with(env_filter)
                .with(fmt::layer())
                .init();
        }
    }

    Ok(())
}

// Structured access log entry (emitted after every request)
#[derive(serde::Serialize)]
pub struct AccessLog {
    pub timestamp: String,
    pub method: String,
    pub path: String,
    pub status: u16,
    pub upstream_latency_ms: u64,
    pub bytes_out: u64,
}
