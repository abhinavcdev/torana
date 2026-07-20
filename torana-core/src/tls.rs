use anyhow::Result;
use rustls_pemfile::{certs, private_key};
use std::fs;
use std::io::BufReader;
use tokio_rustls::{rustls, TlsAcceptor};

pub fn load_tls_config(cert_path: &str, key_path: &str) -> Result<TlsAcceptor> {
    // Read certificate file
    let cert_file = fs::File::open(cert_path)?;
    let mut cert_reader = BufReader::new(cert_file);
    let certs = certs(&mut cert_reader).collect::<Result<Vec<_>, _>>()?;

    if certs.is_empty() {
        anyhow::bail!("No certificates found in {}", cert_path);
    }

    // Read private key file
    let key_file = fs::File::open(key_path)?;
    let mut key_reader = BufReader::new(key_file);
    let key = private_key(&mut key_reader)?
        .ok_or_else(|| anyhow::anyhow!("No private key found in {}", key_path))?;

    // Create ServerConfig with no client auth
    let server_config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)?;

    Ok(TlsAcceptor::from(std::sync::Arc::new(server_config)))
}
