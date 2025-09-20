use anyhow::{Context, Result};
use iroh::{Endpoint, SecretKey, Watcher};
use iroh_base::ticket::NodeTicket;
use tokio::io::{AsyncRead, AsyncWrite};
use tracing::Level;

pub struct Server {
    endpoint: Endpoint,
}

impl Server {
    #[tracing::instrument]
    pub async fn new() -> Result<Self> {
        let endpoint = Endpoint::builder()
            .alpns(["nix-daemon".into()].to_vec())
            .relay_mode(iroh::RelayMode::Default)
            .secret_key(SecretKey::from_bytes(&[
                1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1,
                1, 1, 1, 1,
            ]))
            .bind()
            .await?;
        endpoint.home_relay().initialized().await?;

        Ok(Self { endpoint })
    }

    #[tracing::instrument(skip_all, ret(level = Level::DEBUG))]
    pub async fn address(&self) -> Result<NodeTicket> {
        let node = self.endpoint.node_addr().initialized().await?;
        Ok(NodeTicket::new(node))
    }

    #[tracing::instrument(skip_all, ret(level = Level::DEBUG))]
    pub async fn short_address(&self) -> Result<NodeTicket> {
        let mut node = self.endpoint.node_addr().initialized().await?;
        node.direct_addresses.clear();
        Ok(NodeTicket::new(node))
    }

    #[tracing::instrument(skip_all)]
    pub async fn accept_connection(&self) -> Result<impl AsyncRead + AsyncWrite + Unpin> {
        let incoming = self
            .endpoint
            .accept()
            .await
            .context("failed to establish incoming p2p connection")?;
        let conn = incoming
            .await
            .context("failed to accept incomming p2p connection")?;
        let (tx, rx) = conn
            .accept_bi()
            .await
            .context("unable accept bidirectional stream")?;
        Ok(tokio::io::join(rx, tx))
    }
}

impl From<Endpoint> for Server {
    fn from(endpoint: Endpoint) -> Self {
        Self { endpoint }
    }
}

#[tracing::instrument(skip_all)]
pub async fn proxy_incoming_stream_to_nix_daemon(
    mut stream: impl AsyncRead + AsyncWrite + Unpin,
) -> Result<()> {
    let mut daemon = tokio::net::UnixSocket::new_stream()?
        .connect("/nix/var/nix/daemon-socket/socket")
        .await?;

    tokio::io::copy_bidirectional(&mut stream, &mut daemon).await?;
    Ok(())
}
