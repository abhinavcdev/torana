//! torana-core: the embeddable reverse proxy engine behind the `torana`
//! binary.
//!
//! Two entry points, depending on how much you want torana to own:
//!
//! - [`Server`] runs the whole standalone process: binds every configured
//!   listener, spawns the metrics/health endpoints, handles SIGHUP reloads
//!   and graceful SIGTERM/SIGINT shutdown. This is what the `torana` binary
//!   calls.
//! - [`ProxyEngine`] is the routing-and-forwarding core with none of that
//!   process machinery, for embedding inside a hyper server you already run:
//!   construct one, then call [`ProxyEngine::handle`] from your own
//!   `service_fn`.

#[cfg(feature = "acme")]
pub mod acme;
pub mod config;
pub mod health;
pub mod logging;
pub mod metrics;
#[cfg(feature = "plugins")]
pub mod plugin;
pub mod proxy;
pub mod reload;
pub mod routing;
mod server;
pub mod tls;
pub mod upstream;

pub use config::{load_config, Config};
pub use metrics::Metrics;
pub use server::{init_logging_from_config, ProxyBody, ProxyEngine, Server};
