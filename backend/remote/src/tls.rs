use std::sync::Arc;

use rustls::{
    ServerConfig,
    pki_types::{CertificateDer, PrivateKeyDer},
    version::TLS13,
};

#[derive(Clone)]
pub struct Tls13ServerConfig(Arc<ServerConfig>);

impl Tls13ServerConfig {
    pub(crate) fn into_inner(self) -> Arc<ServerConfig> {
        self.0
    }

    #[cfg(test)]
    pub(crate) fn from_test_config(config: ServerConfig) -> Self {
        Self(Arc::new(config))
    }
}

/// Client証明書には依存せず、TLS 1.3だけを許可するserver設定を構築する。
pub fn tls13_server_config(
    certificate_chain: Vec<CertificateDer<'static>>,
    private_key: PrivateKeyDer<'static>,
) -> Result<Tls13ServerConfig, rustls::Error> {
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let mut config = ServerConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&TLS13])?
        .with_no_client_auth()
        .with_single_cert(certificate_chain, private_key)?;
    config.max_early_data_size = 0;
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(Tls13ServerConfig(Arc::new(config)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rcgen::{CertifiedKey, generate_simple_self_signed};

    #[test]
    fn tls_builder_rejects_an_empty_certificate_chain() {
        let private_key = PrivateKeyDer::Pkcs8(vec![0_u8; 32].into());
        assert!(tls13_server_config(Vec::new(), private_key).is_err());
    }

    #[test]
    fn tls_builder_enables_only_tls13_and_disables_early_data() {
        let CertifiedKey { cert, signing_key } =
            generate_simple_self_signed(["localhost".to_owned()]).unwrap();
        let config = tls13_server_config(
            vec![cert.der().clone()],
            PrivateKeyDer::Pkcs8(signing_key.serialize_der().into()),
        )
        .unwrap()
        .into_inner();

        assert_eq!(config.max_early_data_size, 0);
        assert_eq!(config.alpn_protocols, [b"http/1.1".to_vec()]);
    }
}
