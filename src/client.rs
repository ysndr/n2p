use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use iroh::Endpoint;
use tokio::{
    io::{AsyncRead, AsyncWrite},
    net::UnixSocket,
};
use tracing::error;

pub type NodeTicket = iroh_base::ticket::NodeTicket;
pub type NodeAddr = iroh::NodeAddr;

pub struct Client {
    endpoint: Endpoint,
}

impl Client {
    #[tracing::instrument]
    pub async fn new() -> Result<Self> {
        let endpoint = Endpoint::builder().bind().await?;
        Ok(Self { endpoint })
    }

    #[tracing::instrument(skip_all)]
    pub async fn connect_to_remote(
        &self,
        addr: impl Into<NodeAddr>,
    ) -> Result<impl AsyncRead + AsyncWrite + Unpin> {
        let conn = self.endpoint.connect(addr, b"nix-daemon").await?;
        let (tx, rx) = conn.open_bi().await.context("unable to open uni")?;

        Ok(tokio::io::join(rx, tx))
    }
}

pub fn start_listener(
    server_address: &NodeTicket,
) -> Result<(SelfCleaningSocketFile, tokio::net::UnixListener)> {
    let stream = tokio::net::UnixSocket::new_stream()?;
    let socket_file_guard = SelfCleaningSocketFile::bind_socket(
        &stream,
        std::env::current_dir()?.join(SocketFileName::from(server_address)),
    )?;
    let listener = stream.listen(1)?;
    Ok((socket_file_guard, listener))
}

#[tracing::instrument(skip_all, err)]
pub async fn proxy_incoming_stream_to_remote(
    mut incoming: impl AsyncRead + AsyncWrite + Unpin,
    client: &Client,
    server_address: impl Into<NodeAddr>,
) -> Result<()> {
    let mut remote = client.connect_to_remote(server_address).await?;
    tokio::io::copy_bidirectional(&mut incoming, &mut remote).await?;
    Ok(())
}

struct SocketFileName(String);
impl From<&NodeTicket> for SocketFileName {
    fn from(server_address: &NodeTicket) -> Self {
        Self(format!(
            "{}.sock",
            server_address
                .node_addr()
                .node_id
                .to_string()
                .chars()
                .take(12)
                .collect::<String>()
        ))
    }
}
impl AsRef<Path> for SocketFileName {
    fn as_ref(&self) -> &Path {
        let SocketFileName(name) = self;
        Path::new(name)
    }
}

pub struct SelfCleaningSocketFile(PathBuf);
impl SelfCleaningSocketFile {
    pub fn bind_socket(socket: &UnixSocket, path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        socket.bind(&path)?;
        Ok(Self(path))
    }

    pub fn as_nix_store_url(&self) -> String {
        let SelfCleaningSocketFile(path) = self;
        format!("unix:{}", path.display())
    }
}

impl AsRef<Path> for SelfCleaningSocketFile {
    fn as_ref(&self) -> &Path {
        &self.0
    }
}

impl Drop for SelfCleaningSocketFile {
    fn drop(&mut self) {
        let _err = std::fs::remove_file(&self.0)
            .inspect_err(|err| error!("warn: failed removing socket file: {err}"));
    }
}
