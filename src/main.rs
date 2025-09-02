use std::{fmt::Debug, pin::Pin};

use anyhow::{Context, Result};
use clap::Parser;
use iroh::{
    Endpoint, NodeAddr, PublicKey, SecretKey, Watcher,
    endpoint::{RecvStream, SendStream},
};
use iroh_base::ticket::NodeTicket;
use nix_daemon::{
    Store,
    nix::{DaemonProtocolAdapter, DaemonStore},
};
use tokio::{
    io::{self, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::{UnixSocket, UnixStream},
};

#[derive(Debug, clap::Parser)]
#[command(version, about, long_about = None)]
struct Args {
    #[command(subcommand)]
    mode: Mode,
}

#[derive(Debug, Clone, clap::Subcommand)]
enum Mode {
    Server,
    Client { server_address: NodeTicket },
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    match args.mode {
        Mode::Server => {
            let server = serve_to_remote().await?;

            loop {
                let conn = server.accept().await.context("unable to open uni")?.await?;

                let mut local = tokio::net::UnixSocket::new_stream()?
                    .connect("/nix/var/nix/daemon-socket/socket")
                    .await?;

                let (tx, rx) = conn.accept_bi().await?;
                let mut stream = DuplexP2PStream::new(rx, tx);

                tokio::io::copy_bidirectional(&mut stream, &mut local).await?;
            }
        }
        Mode::Client { server_address } => {
            let stream = tokio::net::UnixSocket::new_stream()?;
            stream.bind("./proxy.sock")?;
            let listener = stream.listen(1)?;

            // let mut proxy = Sending::new(client);

            loop {
                let (mut socket, _) = listener.accept().await?;
                let (rx, tx) = socket.split();
                let mut socket = DuplexP2PStream::new(rx, tx);

                let mut client = connect_to_remote(server_address.node_addr().clone()).await?;
                let result = tokio::io::copy_bidirectional(&mut socket, &mut client).await;

                dbg!(&result);

                if let Err(err) = result.context("oops") {
                    println!("{:#?}", err);
                }
            }
        }
    };
}

struct DuplexP2PStream<R, W>(R, W);
impl<R: AsyncRead, W: AsyncWrite> DuplexP2PStream<R, W> {
    fn new(r: R, w: W) -> Self {
        Self(r, w)
    }
}

impl<R, W> AsyncWrite for DuplexP2PStream<R, W>
where
    R: Unpin,
    W: AsyncWrite + Unpin,
{
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::result::Result<usize, std::io::Error>> {
        AsyncWrite::poll_write(Pin::new(&mut self.get_mut().1), cx, buf)
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::result::Result<(), std::io::Error>> {
        AsyncWrite::poll_flush(Pin::new(&mut self.get_mut().1), cx)
    }

    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::result::Result<(), std::io::Error>> {
        AsyncWrite::poll_shutdown(Pin::new(&mut self.get_mut().1), cx).map(|result| {
            if let Err(ref err) = result
                && err.kind() == tokio::io::ErrorKind::NotConnected
            {
                Ok(())
            } else {
                dbg!(result)
            }
        })
    }
}

impl<R, W> AsyncRead for DuplexP2PStream<R, W>
where
    R: AsyncRead + Unpin,
    W: Unpin,
{
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        AsyncRead::poll_read(Pin::new(&mut self.get_mut().0), cx, buf)
    }
}

async fn connect_to_remote(addr: NodeAddr) -> Result<DuplexP2PStream<RecvStream, SendStream>> {
    let ep = Endpoint::builder()
        // .secret_key(SecretKey::from_bytes(&[
        //     1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1,
        // 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1,
        //     1, 1, 1,
        // ]))
        .bind()
        .await?;
    let conn = ep.connect(addr, b"my-alpn").await?;
    let (tx, rx) = conn.open_bi().await.context("unable to open uni")?;
    let stream = DuplexP2PStream::new(rx, tx);

    Ok(stream)
}

async fn serve_to_remote() -> Result<Endpoint> {
    let ep = Endpoint::builder()
        .alpns(["my-alpn".into()].to_vec())
        .relay_mode(iroh::RelayMode::Default)
        .secret_key(SecretKey::from_bytes(&[
            1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1,
            1, 1, 1,
        ]))
        .bind()
        .await?;
    ep.home_relay().initialized().await?;

    let node = ep.node_addr().initialized().await?;
    let mut short = node.clone();
    let ticket = NodeTicket::new(node);
    short.direct_addresses.clear();
    let short = NodeTicket::new(short);

    println!("address is {ticket}");
    println!("short address is {short}");

    Ok(ep)
}

async fn connect_to_local_daemon() -> Result<DaemonStore<UnixStream>> {
    let conn = DaemonStore::builder()
        .connect_unix("/nix/var/nix/daemon-socket/socket")
        .await?;
    Ok(conn)
}

struct Receiving<C>
where
    C: AsyncWriteExt + AsyncReadExt + Unpin + Send,
{
    local: DaemonStore<C>,
}

impl<C> Receiving<C>
where
    C: AsyncWriteExt + AsyncReadExt + Unpin + Send,
{
    fn new(local: DaemonStore<C>) -> Self {
        Self { local }
    }

    async fn into_daemon_store_adapter<R, W>(
        &mut self,
        listener: DuplexP2PStream<R, W>,
    ) -> Result<DaemonProtocolAdapter<DaemonStore<C>, R, W>>
    where
        R: AsyncRead + Send + Unpin + Debug,
        W: AsyncWrite + Send + Unpin + Debug,
    {
        DaemonProtocolAdapter::builder(&mut self.local)
            .adopt(listener.0, listener.1)
            .await
            .context("unable to adopt listener")
    }
}

struct Sending<R>
where
    R: AsyncWriteExt + AsyncReadExt + Unpin + Send,
{
    remote: DaemonStore<R>,
}

impl<R> Sending<R>
where
    R: AsyncWriteExt + AsyncReadExt + Unpin + Send,
{
    fn new(remote: DaemonStore<R>) -> Self {
        Self { remote }
    }

    async fn into_daemon_store_adapter<CR, CW>(
        &mut self,
        listener: DuplexP2PStream<CR, CW>,
    ) -> Result<DaemonProtocolAdapter<DaemonStore<R>, CR, CW>>
    where
        CR: AsyncRead + Send + Unpin + Debug,
        CW: AsyncWrite + Send + Unpin + Debug,
    {
        DaemonProtocolAdapter::builder(&mut self.remote)
            .adopt(listener.0, listener.1)
            .await
            .context("unable to adopt listener")
    }
}
