use anyhow::{Context, Result};
use rustls_pemfile::{certs, private_key};
use std::fs;
use std::io::BufReader;
use std::sync::Arc;
use tokio_rustls::{rustls, TlsAcceptor};

/// Build a TLS acceptor for a listener. If `client_ca_path` is set, client
/// certificates become mandatory: only connections presenting a certificate
/// signed by that CA are accepted (mTLS). Without it, this is plain TLS
/// termination.
pub fn load_tls_config(
    cert_path: &str,
    key_path: &str,
    client_ca_path: Option<&str>,
) -> Result<TlsAcceptor> {
    let certs = read_certs(cert_path)?;
    if certs.is_empty() {
        anyhow::bail!("No certificates found in {}", cert_path);
    }

    let key_file =
        fs::File::open(key_path).with_context(|| format!("opening key file '{}'", key_path))?;
    let mut key_reader = BufReader::new(key_file);
    let key = private_key(&mut key_reader)?
        .ok_or_else(|| anyhow::anyhow!("No private key found in {}", key_path))?;

    let builder = rustls::ServerConfig::builder();
    let server_config = match client_ca_path {
        Some(ca_path) => {
            let ca_certs = read_certs(ca_path)?;
            if ca_certs.is_empty() {
                anyhow::bail!("No CA certificates found in {}", ca_path);
            }
            let mut roots = rustls::RootCertStore::empty();
            for cert in ca_certs {
                roots
                    .add(cert)
                    .context("adding CA certificate to trust store")?;
            }
            let verifier = rustls::server::WebPkiClientVerifier::builder(Arc::new(roots))
                .build()
                .context("building mTLS client certificate verifier")?;
            builder
                .with_client_cert_verifier(verifier)
                .with_single_cert(certs, key)?
        }
        None => builder.with_no_client_auth().with_single_cert(certs, key)?,
    };

    Ok(TlsAcceptor::from(Arc::new(server_config)))
}

fn read_certs(path: &str) -> Result<Vec<rustls::pki_types::CertificateDer<'static>>> {
    let file =
        fs::File::open(path).with_context(|| format!("opening certificate file '{}'", path))?;
    let mut reader = BufReader::new(file);
    certs(&mut reader)
        .collect::<Result<Vec<_>, _>>()
        .with_context(|| format!("parsing certificates in '{}'", path))
}

/// SHA-256 fingerprint of a peer certificate's DER bytes, hex-encoded. Used
/// to give the upstream a stable identifier for the mTLS-verified client
/// without needing a full X.509 subject parser.
pub fn cert_fingerprint(der: &[u8]) -> String {
    let digest = ring::digest::digest(&ring::digest::SHA256, der);
    digest
        .as_ref()
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect()
}
