use std::{env::args, fmt::Debug, pin::Pin};

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
            let local = connect_to_local_daemon().await?;
            let mut local = Receiving::new(local);

            let server = serve_to_remote().await?;

            loop {
                let conn = server.accept().await.context("unable to open uni")?.await?;
                let (tx, rx) = conn.accept_bi().await?;
                let stream = DuplexP2PStream::new(rx, tx);

                let mut proxy = local.into_daemon_store_adapter(stream).await?;
                match proxy.run().await {
                    Ok(()) => continue,
                    Err(ref err @ nix_daemon::Error::IO(ref io))
                        if io.kind() == io::ErrorKind::NotConnected =>
                    {
                        dbg!(err);
                        continue;
                    }
                    Err(err) => Err(err)?,
                }
            }
        }
        Mode::Client { server_address } => {
            let stream = tokio::net::UnixSocket::new_stream()?;
            stream.bind("./proxy.sock")?;
            let listener = stream.listen(128)?;

            let client = connect_to_remote(server_address.node_addr().clone()).await?;
            let mut proxy = Sending::new(client);

            loop {
                let (mut socket, _) = listener.accept().await?;
                let (rx, tx) = socket.split();
                let mut proxy = proxy
                    .into_daemon_store_adapter(DuplexP2PStream(rx, tx))
                    .await?;
                proxy.run().await?;
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
        AsyncWrite::poll_shutdown(Pin::new(&mut self.get_mut().1), cx)
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

async fn connect_to_remote(
    addr: NodeAddr,
) -> Result<DaemonStore<DuplexP2PStream<RecvStream, SendStream>>> {
    let ep = Endpoint::builder()
        // .secret_key(SecretKey::from_bytes(&[
        //     1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1,
        //     1, 1, 1,
        // ]))
        .bind()
        .await?;
    let conn = ep.connect(addr, b"my-alpn").await?;
    let (tx, rx) = conn.open_bi().await.context("unable to open uni")?;
    let stream = DuplexP2PStream::new(rx, tx);

    let conn = DaemonStore::builder()
        .init(stream)
        .await
        .context("unable to connect to daemon")?;

    Ok(conn)
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
    // short.direct_addresses.clear();
    // let short = NodeTicket::new(short);

    println!("address is {ticket}");

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

// impl<C> Store for Receiving<C>
// where
//     DaemonStore<C>: Store,
//     C: AsyncWriteExt + AsyncReadExt + Unpin + Send,
// {
//     type Error = <DaemonStore<C> as Store>::Error;

//     fn is_valid_path<P: AsRef<str> + Send + Sync + std::fmt::Debug>(
//         &mut self,
//         path: P,
//     ) -> impl nix_daemon::Progress<T = bool, Error = Self::Error> {
//         self.local.is_valid_path(path)
//     }

//     fn has_substitutes<P: AsRef<str> + Send + Sync + std::fmt::Debug>(
//         &mut self,
//         path: P,
//     ) -> impl nix_daemon::Progress<T = bool, Error = Self::Error> {
//         self.local.has_substitutes(path)
//     }

//     fn add_to_store<
//         SN: AsRef<str> + Send + Sync + std::fmt::Debug,
//         SC: AsRef<str> + Send + Sync + std::fmt::Debug,
//         Refs,
//         R,
//     >(
//         &mut self,
//         name: SN,
//         cam_str: SC,
//         refs: Refs,
//         repair: bool,
//         source: R,
//     ) -> impl nix_daemon::Progress<T = (String, nix_daemon::PathInfo), Error = Self::Error>
//     where
//         Refs: IntoIterator + Send + std::fmt::Debug,
//         Refs::IntoIter: ExactSizeIterator + Send,
//         Refs::Item: AsRef<str> + Send + Sync,
//         R: AsyncReadExt + Unpin + Send + std::fmt::Debug,
//     {
//         self.local.add_to_store(name, cam_str, refs, repair, source)
//     }

//     fn build_paths<Paths>(
//         &mut self,
//         paths: Paths,
//         mode: nix_daemon::BuildMode,
//     ) -> impl nix_daemon::Progress<T = (), Error = Self::Error>
//     where
//         Paths: IntoIterator + Send + std::fmt::Debug,
//         Paths::IntoIter: ExactSizeIterator + Send,
//         Paths::Item: AsRef<str> + Send + Sync,
//     {
//         self.local.build_paths(paths, mode)
//     }

//     fn ensure_path<Path: AsRef<str> + Send + Sync + std::fmt::Debug>(
//         &mut self,
//         path: Path,
//     ) -> impl nix_daemon::Progress<T = (), Error = Self::Error> {
//         self.local.ensure_path(path)
//     }

//     fn add_temp_root<Path: AsRef<str> + Send + Sync + std::fmt::Debug>(
//         &mut self,
//         path: Path,
//     ) -> impl nix_daemon::Progress<T = (), Error = Self::Error> {
//         self.local.add_temp_root(path)
//     }

//     fn add_indirect_root<Path: AsRef<str> + Send + Sync + std::fmt::Debug>(
//         &mut self,
//         path: Path,
//     ) -> impl nix_daemon::Progress<T = (), Error = Self::Error> {
//         self.local.add_indirect_root(path)
//     }

//     fn find_roots(
//         &mut self,
//     ) -> impl nix_daemon::Progress<T = std::collections::HashMap<String, String>, Error = Self::Error>
//     {
//         self.local.find_roots()
//     }

//     fn set_options(
//         &mut self,
//         opts: nix_daemon::ClientSettings,
//     ) -> impl nix_daemon::Progress<T = (), Error = Self::Error> {
//         self.local.set_options(opts)
//     }

//     fn query_pathinfo<S: AsRef<str> + Send + Sync + std::fmt::Debug>(
//         &mut self,
//         path: S,
//     ) -> impl nix_daemon::Progress<T = Option<nix_daemon::PathInfo>, Error = Self::Error> {
//         self.local.query_pathinfo(path)
//     }

//     fn query_valid_paths<Paths>(
//         &mut self,
//         paths: Paths,
//         use_substituters: bool,
//     ) -> impl nix_daemon::Progress<T = Vec<String>, Error = Self::Error>
//     where
//         Paths: IntoIterator + Send + std::fmt::Debug,
//         Paths::IntoIter: ExactSizeIterator + Send,
//         Paths::Item: AsRef<str> + Send + Sync,
//     {
//         self.local.query_valid_paths(paths, use_substituters)
//     }

//     fn query_substitutable_paths<Paths>(
//         &mut self,
//         paths: Paths,
//     ) -> impl nix_daemon::Progress<T = Vec<String>, Error = Self::Error>
//     where
//         Paths: IntoIterator + Send + std::fmt::Debug,
//         Paths::IntoIter: ExactSizeIterator + Send,
//         Paths::Item: AsRef<str> + Send + Sync,
//     {
//         self.local.query_substitutable_paths(paths)
//     }

//     fn query_valid_derivers<S: AsRef<str> + Send + Sync + std::fmt::Debug>(
//         &mut self,
//         path: S,
//     ) -> impl nix_daemon::Progress<T = Vec<String>, Error = Self::Error> {
//         self.local.query_valid_derivers(path)
//     }

//     fn query_missing<Ps>(
//         &mut self,
//         paths: Ps,
//     ) -> impl nix_daemon::Progress<T = nix_daemon::Missing, Error = Self::Error>
//     where
//         Ps: IntoIterator + Send + std::fmt::Debug,
//         Ps::IntoIter: ExactSizeIterator + Send,
//         Ps::Item: AsRef<str> + Send + Sync,
//     {
//         self.local.query_missing(paths)
//     }

//     fn query_derivation_output_map<P: AsRef<str> + Send + Sync + std::fmt::Debug>(
//         &mut self,
//         path: P,
//     ) -> impl nix_daemon::Progress<T = std::collections::HashMap<String, String>, Error = Self::Error>
//     {
//         self.local.query_derivation_output_map(path)
//     }

//     fn build_paths_with_results<Ps>(
//         &mut self,
//         paths: Ps,
//         mode: nix_daemon::BuildMode,
//     ) -> impl nix_daemon::Progress<
//         T = std::collections::HashMap<String, nix_daemon::BuildResult>,
//         Error = Self::Error,
//     >
//     where
//         Ps: IntoIterator + Send + std::fmt::Debug,
//         Ps::IntoIter: ExactSizeIterator + Send,
//         Ps::Item: AsRef<str> + Send + Sync,
//     {
//         self.local.build_paths_with_results(paths, mode)
//     }
// }
