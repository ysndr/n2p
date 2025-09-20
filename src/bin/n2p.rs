use std::{fmt::Debug, str::FromStr};

use anyhow::{Context, Result};
use clap::Parser;

use n2p::{
    client::{Client, NodeTicket, proxy_incoming_stream_to_remote, start_listener},
    server::{Server, proxy_incoming_stream_to_nix_daemon},
};
use tokio::select;
use tracing::info;
use tracing_subscriber::EnvFilter;

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

    let default_filter = EnvFilter::from_str("n2p=info").unwrap();
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::builder()
                .try_from_env()
                .unwrap_or(default_filter),
        )
        .init();

    select! {
        _ = tokio::spawn(run(args)) => {},
        _ = tokio::spawn(tokio::signal::ctrl_c()) => {}
    }

    Ok(())
}

async fn run(args: Args) -> Result<()> {
    match args.mode {
        Mode::Server => server().await?,
        Mode::Client { server_address } => client(server_address).await?,
    }
    Ok(())
}

async fn server() -> Result<()> {
    let p2p_server = Server::new().await.context("could not create p2p server")?;

    let address = p2p_server.address().await?;
    let short_address = p2p_server.short_address().await?;

    info!(%address, %short_address, "server started");

    loop {
        let stream = p2p_server.accept_connection().await?;
        let _ = proxy_incoming_stream_to_nix_daemon(stream).await;
    }
}

async fn client(server_address: NodeTicket) -> Result<()> {
    let (socket_file_guard, listener) = start_listener(&server_address)?;
    let p2p_client = Client::new().await?;

    println!("{}", socket_file_guard.as_nix_store_url());

    loop {
        let (local_stream, _) = listener.accept().await?;
        let _ = proxy_incoming_stream_to_remote(local_stream, &p2p_client, server_address.clone())
            .await;
    }
}
