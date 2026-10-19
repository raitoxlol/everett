use std::{collections::BTreeMap, path::Path, sync::Arc};

use convex::{ConvexClient, ConvexClientBuilder, FunctionResult, WebSocketState};
use rustls::{pki_types::pem::PemObject, pki_types::CertificateDer, ClientConfig, RootCertStore};
use tokio::sync::mpsc;
use tokio_tungstenite::Connector;

pub(super) struct Client {
    inner: ConvexClient,
    states: mpsc::Receiver<WebSocketState>,
}

fn tls_config(ca_file: &str) -> anyhow::Result<Arc<ClientConfig>> {
    let mut roots = RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    if !ca_file.is_empty() {
        let certificates = CertificateDer::pem_file_iter(Path::new(ca_file))
            .map_err(|e| anyhow::anyhow!("cannot read relay CA file {ca_file}: {e}"))?;
        let mut count = 0;
        for certificate in certificates {
            roots
                .add(
                    certificate
                        .map_err(|e| anyhow::anyhow!("invalid relay CA file {ca_file}: {e}"))?,
                )
                .map_err(|e| anyhow::anyhow!("invalid relay CA file {ca_file}: {e}"))?;
            count += 1;
        }
        anyhow::ensure!(
            count > 0,
            "relay CA file {ca_file} contains no PEM certificates"
        );
    }
    let config =
        ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()?
            .with_root_certificates(roots)
            .with_no_client_auth();
    Ok(Arc::new(config))
}

impl Client {
    pub async fn new(url: &str) -> anyhow::Result<Self> {
        let ca_file = crate::config::get("relay.ca_file", Some("EVERETT_RELAY_CA_FILE"), "");
        Self::with_ca_file(url, &ca_file).await
    }

    async fn with_ca_file(url: &str, ca_file: &str) -> anyhow::Result<Self> {
        let (sender, states) = mpsc::channel(32);
        let inner = ConvexClientBuilder::new(url)
            .with_connector(Connector::Rustls(tls_config(ca_file)?))
            .with_on_state_change(sender)
            .build()
            .await?;
        Ok(Self { inner, states })
    }

    pub async fn tls_error(&mut self) -> anyhow::Error {
        loop {
            match self.states.recv().await {
                Some(WebSocketState::TlsError(error)) => return anyhow::anyhow!(
                    "relay TLS/certificate verification failed: {error}. For a private CA, explicitly set EVERETT_RELAY_CA_FILE or [relay] ca_file to its PEM file. TLS verification remains enabled."
                ),
                Some(_) => {},
                None => return anyhow::anyhow!("Convex connection worker stopped"),
            }
        }
    }

    pub async fn query(
        &mut self,
        name: &str,
        args: BTreeMap<String, convex::Value>,
    ) -> anyhow::Result<FunctionResult> {
        let mut inner = self.inner.clone();
        tokio::select! {
            biased;
            error = self.tls_error() => Err(error),
            result = inner.query(name, args) => result,
        }
    }

    pub async fn mutation(
        &mut self,
        name: &str,
        args: BTreeMap<String, convex::Value>,
    ) -> anyhow::Result<FunctionResult> {
        let mut inner = self.inner.clone();
        tokio::select! {
            biased;
            error = self.tls_error() => Err(error),
            result = inner.mutation(name, args) => result,
        }
    }
}

impl std::ops::Deref for Client {
    type Target = ConvexClient;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl std::ops::DerefMut for Client {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;
    use rustls::{pki_types::PrivateKeyDer, ServerConfig};
    use std::{fs, process::Command, time::Duration};
    use tokio::{net::TcpListener, time::timeout};
    use tokio_rustls::TlsAcceptor;

    fn certificate() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("openssl.cnf"), "[req]\ndistinguished_name=dn\nx509_extensions=ca\nprompt=no\n[dn]\nCN=Test CA\n[ca]\nbasicConstraints=critical,CA:TRUE\nkeyUsage=critical,keyCertSign\n[server]\nbasicConstraints=critical,CA:FALSE\nsubjectAltName=DNS:localhost\nextendedKeyUsage=serverAuth\n").unwrap();
        for args in [
            vec![
                "req",
                "-x509",
                "-newkey",
                "rsa:2048",
                "-nodes",
                "-days",
                "1",
                "-keyout",
                "ca.key",
                "-out",
                "ca.pem",
                "-config",
                "openssl.cnf",
            ],
            vec![
                "req",
                "-new",
                "-newkey",
                "rsa:2048",
                "-nodes",
                "-keyout",
                "server.key",
                "-out",
                "server.csr",
                "-subj",
                "/CN=localhost",
            ],
            vec![
                "x509",
                "-req",
                "-in",
                "server.csr",
                "-CA",
                "ca.pem",
                "-CAkey",
                "ca.key",
                "-CAcreateserial",
                "-days",
                "1",
                "-out",
                "server.pem",
                "-extfile",
                "openssl.cnf",
                "-extensions",
                "server",
            ],
        ] {
            let output = Command::new("openssl")
                .args(args)
                .current_dir(dir.path())
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        dir
    }

    async fn server(dir: &Path) -> (u16, tokio::task::JoinHandle<()>) {
        let certificates = CertificateDer::pem_file_iter(dir.join("server.pem"))
            .unwrap()
            .collect::<std::result::Result<Vec<_>, _>>()
            .unwrap();
        let key = PrivateKeyDer::from_pem_file(dir.join("server.key")).unwrap();
        let config =
            ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
                .with_safe_default_protocol_versions()
                .unwrap()
                .with_no_client_auth()
                .with_single_cert(certificates, key)
                .unwrap();
        let acceptor = TlsAcceptor::from(Arc::new(config));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let task = tokio::spawn(async move {
            loop {
                let (stream, _) = listener.accept().await.unwrap();
                if let Ok(stream) = acceptor.accept(stream).await {
                    let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
                    assert!(socket.next().await.unwrap().is_ok());
                    // Close after the SDK's Connect message to exercise reconnects.
                    socket.close(None).await.unwrap();
                }
            }
        });
        (port, task)
    }

    #[tokio::test]
    async fn explicit_ca_connects_and_reconnects_without_global_trust() {
        let dir = certificate();
        let (port, task) = server(dir.path()).await;
        let mut client = Client::with_ca_file(
            &format!("https://localhost:{port}"),
            dir.path().join("ca.pem").to_str().unwrap(),
        )
        .await
        .unwrap();
        timeout(Duration::from_secs(8), async {
            for _ in 0..2 {
                loop {
                    match client.states.recv().await.unwrap() {
                        WebSocketState::Connected => break,
                        WebSocketState::TlsError(e) => panic!("trusted CA rejected: {e}"),
                        _ => {}
                    }
                }
            }
        })
        .await
        .expect("trusted CA must work on first connection and reconnect");
        task.abort();
    }

    #[tokio::test]
    async fn default_roots_and_unrelated_ca_reject_private_server_promptly() {
        let dir = certificate();
        let unrelated = certificate();
        let (port, task) = server(dir.path()).await;
        let other = unrelated.path().join("ca.pem");
        for ca in ["", other.to_str().unwrap()] {
            let mut client = Client::with_ca_file(&format!("https://localhost:{port}"), ca)
                .await
                .unwrap();
            let error = timeout(
                Duration::from_secs(5),
                client.query("test:query", BTreeMap::new()),
            )
            .await
            .expect("TLS must fail before the request deadline")
            .unwrap_err();
            let error = format!("{error:#}");
            assert!(
                error.contains("TLS/certificate verification failed"),
                "{error}"
            );
            assert!(error.contains("invalid peer certificate"), "{error}");
            assert!(
                error.contains("UnknownIssuer") || error.contains("BadSignature"),
                "{error}"
            );
            assert!(error.contains("EVERETT_RELAY_CA_FILE"), "{error}");
        }
        task.abort();
    }

    #[tokio::test]
    async fn trusted_ca_still_requires_valid_server_name() {
        let dir = certificate();
        let (port, task) = server(dir.path()).await;
        let mut client = Client::with_ca_file(
            &format!("https://127.0.0.1:{port}"),
            dir.path().join("ca.pem").to_str().unwrap(),
        )
        .await
        .unwrap();
        let error = timeout(
            Duration::from_secs(5),
            client.mutation("test:mutation", BTreeMap::new()),
        )
        .await
        .unwrap()
        .unwrap_err()
        .to_string();
        assert!(
            error.contains("TLS/certificate verification failed"),
            "{error}"
        );
        assert!(error.contains("certificate not valid for name"), "{error}");
        task.abort();
    }

    #[test]
    fn missing_empty_and_malformed_ca_files_fail_closed() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("ca.pem");
        assert!(tls_config(file.to_str().unwrap())
            .unwrap_err()
            .to_string()
            .contains("cannot read relay CA file"));
        for content in [
            "",
            "not PEM",
            "-----BEGIN CERTIFICATE-----\ninvalid\n-----END CERTIFICATE-----\n",
        ] {
            fs::write(&file, content).unwrap();
            let error = tls_config(file.to_str().unwrap()).unwrap_err().to_string();
            assert!(error.contains("relay CA file"), "{error}");
        }
    }
}
