//! Automatic HTTPS via ACME (RFC 8555), using TLS-ALPN-01 validation.
//!
//! Wraps [`rustls_acme`], which handles certificate issuance/renewal and the
//! TLS-ALPN-01 challenge handshake. torana keeps its own per-listener accept
//! loop and connection-serving code unchanged; only how the TLS handshake is
//! completed differs for an ACME-managed listener.

use anyhow::Context;
use rustls_acme::caches::DirCache;
use rustls_acme::rustls;
use rustls_acme::{AcmeAcceptor, AcmeConfig, AcmeState};
use std::sync::Arc;

pub type AcmeEventState = AcmeState<std::io::Error, std::io::Error>;

pub struct AcmeHandle {
    pub state: AcmeEventState,
    pub acceptor: AcmeAcceptor,
    pub rustls_config: Arc<rustls::ServerConfig>,
}

/// Build the ACME state machine, the challenge-aware acceptor, and the
/// rustls config used to complete ordinary (non-challenge) connections.
/// Does not perform any network I/O itself — issuance happens lazily as
/// connections arrive and is driven by polling `state` as a stream.
///
/// `directory_url`, if set (e.g. pointing at a local Pebble instance for
/// testing), always wins over `staging`.
pub fn build(
    domains: Vec<String>,
    contact_emails: Vec<String>,
    cache_dir: String,
    directory_url: Option<String>,
    staging: bool,
    ca_cert_path: Option<String>,
) -> anyhow::Result<AcmeHandle> {
    if domains.is_empty() {
        anyhow::bail!("ACME requires at least one domain");
    }

    let mut config = AcmeConfig::new(domains).cache(DirCache::new(cache_dir));
    for email in contact_emails {
        config = config.contact_push(format!("mailto:{}", email));
    }
    config = match directory_url {
        Some(url) => config.directory(url),
        None => config.directory_lets_encrypt(!staging),
    };
    if let Some(ca_cert_path) = ca_cert_path {
        let client_config = build_client_tls_config(&ca_cert_path)?;
        tracing::info!(
            ca_cert_path,
            "acme: installed custom trust root for ACME directory"
        );
        config = config.client_tls_config(Arc::new(client_config));
    }

    let state: AcmeEventState = config.state();
    // `AcmeState::acceptor` is soft-deprecated in favor of the crate's
    // high-level `Incoming` stream, which wants to own the TCP-accept loop
    // itself. That doesn't fit torana's per-listener accept loop (shared
    // with the static-cert and mTLS paths), and the crate's suggested
    // low-level replacement (`tokio_rustls::LazyConfigAcceptor` fed with
    // `AcmeState`'s rustls config) only type-checks against a tokio-rustls
    // pinned to rustls 0.22 — one major version behind the rustls 0.23 this
    // crate otherwise uses. This low-level call remains fully supported and
    // is the correct integration point for this dependency combination.
    #[allow(deprecated)]
    let acceptor = state.acceptor();
    let resolver = state.resolver();

    let mut rustls_config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_cert_resolver(resolver);
    // Same client-facing protocol preference as the static-cert path.
    rustls_config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];

    Ok(AcmeHandle {
        state,
        acceptor,
        rustls_config: Arc::new(rustls_config),
    })
}

/// Builds a client TLS config trusting only the given CA, for verifying the
/// ACME directory server itself — used for a private ACME CA, or a local
/// test server like Pebble whose root isn't in the public web PKI.
fn build_client_tls_config(ca_cert_path: &str) -> anyhow::Result<rustls::ClientConfig> {
    let file = std::fs::File::open(ca_cert_path)
        .with_context(|| format!("opening acme.ca_cert '{}'", ca_cert_path))?;
    let mut reader = std::io::BufReader::new(file);
    let certs: Vec<_> = rustls_pemfile::certs(&mut reader)
        .collect::<Result<Vec<_>, _>>()
        .with_context(|| format!("parsing acme.ca_cert '{}'", ca_cert_path))?;
    if certs.is_empty() {
        anyhow::bail!("acme.ca_cert '{}' contains no certificates", ca_cert_path);
    }
    let mut roots = rustls::RootCertStore::empty();
    for cert in certs {
        roots
            .add(cert)
            .context("adding acme.ca_cert to trust store")?;
    }
    Ok(rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth())
}

/// Drives certificate issuance/renewal; must run for the lifetime of the
/// listener. Logs each ACME event (order requests, challenge validation,
/// issuance, renewal) rather than propagating errors, since a transient
/// ACME hiccup should not take down already-served TLS traffic.
pub async fn drive(mut state: AcmeEventState) {
    use futures::StreamExt;
    while let Some(event) = state.next().await {
        match event {
            Ok(ok) => tracing::info!("acme: {:?}", ok),
            Err(e) => tracing::warn!("acme error: {:?}", e),
        }
    }
}

/// One accepted, application-ready ACME-managed connection: a tokio-
/// compatible TLS stream plus whether it negotiated h2 via ALPN.
pub struct AcceptedConn<IO> {
    pub stream: tokio_util::compat::Compat<rustls_acme::futures_rustls::server::TlsStream<IO>>,
    pub use_h2: bool,
}

/// Accept one connection on an ACME-managed listener: peeks the ClientHello
/// to route TLS-ALPN-01 validation traffic internally, and completes real
/// application traffic with `rustls_config`. Returns `Ok(None)` when the
/// connection was a validation handshake fully handled internally (nothing
/// left for the caller to serve).
pub async fn accept<IO>(
    acceptor: &AcmeAcceptor,
    io: IO,
    rustls_config: Arc<rustls::ServerConfig>,
) -> std::io::Result<Option<AcceptedConn<IO>>>
where
    IO: futures::AsyncRead + futures::AsyncWrite + Unpin,
{
    use tokio_util::compat::FuturesAsyncReadCompatExt;
    match acceptor.accept(io).await? {
        None => Ok(None),
        Some(start_handshake) => {
            let tls_stream = start_handshake
                .into_stream(rustls_config)
                .await
                .context("completing ACME-managed TLS handshake")
                .map_err(std::io::Error::other)?;
            let use_h2 = tls_stream.get_ref().1.alpn_protocol() == Some(b"h2");
            Ok(Some(AcceptedConn {
                stream: tls_stream.compat(),
                use_h2,
            }))
        }
    }
}
