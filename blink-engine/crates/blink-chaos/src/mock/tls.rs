//! Self-signed cert helpers shared by `MockClobServer` and any TLS
//! mock that lands later.

use std::sync::Arc;

use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};

/// Result of [`gen_cert`].
pub struct TestCert {
    /// DER-encoded self-signed cert (pass to the client trust store).
    pub cert: CertificateDer<'static>,
    /// Matching PKCS#8 private key (pass to the server config).
    pub key: PrivateKeyDer<'static>,
}

/// Generate a fresh self-signed cert for `localhost`.
pub fn gen_cert() -> TestCert {
    let ca = rcgen::generate_simple_self_signed(vec!["localhost".into()])
        .expect("rcgen self-signed cert");
    let cert = CertificateDer::from(ca.cert.der().to_vec());
    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(ca.key_pair.serialize_der()));
    TestCert { cert, key }
}

/// Build a `rustls::ClientConfig` that trusts a single cert and speaks
/// only H2. Matches the shape `blink-h2` expects.
pub fn client_config_trusting(cert: CertificateDer<'static>) -> Arc<rustls::ClientConfig> {
    let mut roots = rustls::RootCertStore::empty();
    roots.add(cert).expect("add root");
    let mut cfg = rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    blink_h2::tune_resumption(&mut cfg);
    cfg.alpn_protocols = vec![b"h2".to_vec()];
    Arc::new(cfg)
}

/// Build a `rustls::ServerConfig` pinned to a single cert + key and
/// H2 ALPN.
pub fn server_config(cert: CertificateDer<'static>, key: PrivateKeyDer<'static>) -> Arc<rustls::ServerConfig> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let mut cfg = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![cert], key)
        .expect("server cert");
    cfg.alpn_protocols = vec![b"h2".to_vec()];
    Arc::new(cfg)
}
