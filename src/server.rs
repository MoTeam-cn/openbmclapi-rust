//! HTTP/1.1 + HTTP/2 server built on hyper's auto-detecting connection driver.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::Router;
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto::Builder as AutoBuilder;
use hyper_util::service::TowerToHyperService;
use tokio::net::TcpListener;
use tokio::sync::watch;
use tokio_rustls::TlsAcceptor;
use tower::Service as _;
use tracing::{debug, info, warn};

use crate::error::{Error, Result};

/// The peer address of a connection, stamped onto every request it carries so
/// the access log can name the client the way morgan's `combined` format does.
#[derive(Debug, Clone, Copy)]
pub struct PeerAddr(pub SocketAddr);

/// A bound listener plus the router that serves it.
pub struct HttpServer {
    listener: TcpListener,
    router: Router,
    tls: Option<TlsAcceptor>,
    shutdown: watch::Sender<bool>,
}

impl HttpServer {
    /// Bind the listening socket. `tls` enables HTTPS with HTTP/2 negotiation.
    pub async fn bind(
        addr: &str,
        router: Router,
        tls: Option<Arc<rustls::ServerConfig>>,
    ) -> Result<Self> {
        let listener = TcpListener::bind(addr)
            .await
            .map_err(|e| Error::Other(format!("cannot bind {addr}: {e}")))?;
        let (shutdown, _) = watch::channel(false);
        Ok(HttpServer {
            listener,
            router,
            tls: tls.map(TlsAcceptor::from),
            shutdown,
        })
    }

    /// Local address the listener is bound to.
    pub fn local_addr(&self) -> Result<std::net::SocketAddr> {
        self.listener
            .local_addr()
            .map_err(|e| Error::Other(format!("cannot read local address: {e}")))
    }

    /// Ask the accept loop to stop.
    pub fn close(&self) {
        let _ = self.shutdown.send(true);
    }

    /// Accept connections until `close` is called.
    pub async fn serve(self: Arc<Self>) -> Result<()> {
        let mut shutdown = self.shutdown.subscribe();
        info!(addr = %self.listener.local_addr().map(|a| a.to_string()).unwrap_or_default(), tls = self.tls.is_some(), "开始监听");

        loop {
            tokio::select! {
                changed = shutdown.changed() => {
                    if changed.is_ok() {
                        break;
                    }
                    break;
                }
                accepted = self.listener.accept() => {
                    let (stream, peer) = match accepted {
                        Ok(pair) => pair,
                        Err(e) => {
                            warn!(error = %e, "接受连接失败");
                            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                            continue;
                        }
                    };
                    let acceptor = self.tls.clone();
                    let router = self.router.clone();
                    tokio::spawn(async move {
                        let service = TowerToHyperService::new(tower::service_fn(
                            move |mut request: hyper::Request<hyper::body::Incoming>| {
                                request.extensions_mut().insert(PeerAddr(peer));
                                let mut router = router.clone();
                                async move { router.call(request).await }
                            },
                        ));
                        let builder = AutoBuilder::new(TokioExecutor::new());
                        match acceptor {
                            Some(acceptor) => match acceptor.accept(stream).await {
                                Ok(tls_stream) => {
                                    let io = TokioIo::new(tls_stream);
                                    if let Err(e) = builder.serve_connection_with_upgrades(io, service).await {
                                        debug!(%peer, error = %e, "TLS 连接结束");
                                    }
                                }
                                Err(e) => debug!(%peer, error = %e, "TLS 握手失败"),
                            },
                            None => {
                                let io = TokioIo::new(stream);
                                if let Err(e) = builder.serve_connection_with_upgrades(io, service).await {
                                    debug!(%peer, error = %e, "连接结束");
                                }
                            }
                        }
                    });
                }
            }
        }
        debug!("监听已关闭");
        Ok(())
    }
}

/// Build a rustls server configuration from PEM material, preferring HTTP/2.
pub fn tls_config(cert_pem: &str, key_pem: &str) -> Result<Arc<rustls::ServerConfig>> {
    let certificates = crate::tls::parse_certificates(cert_pem)?;
    let key = crate::tls::parse_private_key(key_pem)?;
    let mut config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certificates, key)
        .map_err(|e| Error::Config(format!("TLS 证书无效：{e}")))?;
    config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    Ok(Arc::new(config))
}

/// Install the process-wide rustls crypto provider exactly once.
pub fn install_crypto_provider() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        // `ring` is always compiled in through the websocket stack, so prefer it
        // and fall back to whichever provider rustls selected by default.
        if rustls::crypto::ring::default_provider()
            .install_default()
            .is_err()
        {
            debug!("rustls 加密提供者已安装");
        }
    });
}
