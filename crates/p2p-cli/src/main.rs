use anyhow::{anyhow, Context, Result};
use clap::{Parser, Subcommand};
use once_cell::sync::Lazy;
use p2p_node::{identity::NodeIdentity, net::NodeConfig, Node};
use std::fs;
use std::net::SocketAddr;
use std::path::PathBuf;
use tokio::io::{self, AsyncBufReadExt, BufReader};
use tracing::{error, info};
use tracing_subscriber::EnvFilter;

static LOGGING: Lazy<()> = Lazy::new(|| {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .with_target(false)
        .init();
});

#[derive(Parser, Debug)]
#[command(name = "p2p-chat", version, about = "Rust P2P chat node CLI")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    Run {
        #[arg(long, default_value = "/ip4/0.0.0.0/tcp/0")]
        listen: String,
        #[arg(long, default_value = "0.0.0.0:9898")]
        metrics: SocketAddr,
        #[arg(long)]
        bootstrap: Vec<String>,
        #[arg(long, default_value = "strict")]
        validation: String,
        #[arg(long, default_value_t = 3)]
        kad_parallelism: u8,
        #[arg(long)]
        quic_only: bool,
        #[arg(long)]
        tcp_only: bool,
        #[arg(long)]
        disable_udp: bool,
        #[arg(long, default_value_t = 0)]
        udp_port: u16,
        #[arg(long, value_name = "PATH")]
        identity: Option<PathBuf>,
        #[arg(long, value_name = "PATH")]
        identity_out: Option<PathBuf>,
        #[arg(long, value_name = "SOCKET")]
        stun_server: Vec<SocketAddr>,
        #[arg(long)]
        external: Vec<String>,
        #[arg(long)]
        publish: Option<String>,
        #[arg(long)]
        no_stdin: bool,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    Lazy::force(&LOGGING);
    let cli = Cli::parse();
    match cli.command {
        Commands::Run {
            listen,
            metrics,
            bootstrap,
            validation,
            kad_parallelism,
            quic_only,
            tcp_only,
            disable_udp,
            udp_port,
            identity,
            identity_out,
            stun_server,
            external,
            publish,
            no_stdin,
        } => {
            run_node(
                listen,
                metrics,
                bootstrap,
                validation,
                kad_parallelism,
                quic_only,
                tcp_only,
                disable_udp,
                udp_port,
                identity,
                identity_out,
                stun_server,
                external,
                publish,
                no_stdin,
            )
            .await
        }
    }
}

async fn run_node(
    listen: String,
    metrics: SocketAddr,
    bootstrap: Vec<String>,
    validation: String,
    kad_parallelism: u8,
    quic_only: bool,
    tcp_only: bool,
    disable_udp: bool,
    udp_port: u16,
    identity_path: Option<PathBuf>,
    identity_out: Option<PathBuf>,
    stun_server: Vec<SocketAddr>,
    external: Vec<String>,
    publish: Option<String>,
    no_stdin: bool,
) -> Result<()> {
    let listen_addr = listen.parse()?;
    let mut config = NodeConfig::default();
    config.local_bind_addr = listen_addr;
    config.metrics_addr = metrics;
    config.bootstrap = bootstrap
        .into_iter()
        .map(|addr| addr.parse())
        .collect::<Result<Vec<_>, _>>()?;
    config.gossip_validation_mode = match validation.as_str() {
        "strict" => libp2p::gossipsub::ValidationMode::Strict,
        "anonymous" => libp2p::gossipsub::ValidationMode::Anonymous,
        "none" => libp2p::gossipsub::ValidationMode::None,
        other => anyhow::bail!("unknown validation mode: {other}"),
    };
    config.kad_parallelism = kad_parallelism;
    match (quic_only, tcp_only) {
        (true, true) => anyhow::bail!("cannot set both --quic-only and --tcp-only"),
        (true, false) => {
            config.enable_quic = true;
            config.enable_tcp = false;
        }
        (false, true) => {
            config.enable_quic = false;
            config.enable_tcp = true;
        }
        (false, false) => {
            config.enable_quic = true;
            config.enable_tcp = true;
        }
    }

    config.enable_udp = !disable_udp;
    config.udp_port = udp_port;

    if !stun_server.is_empty() {
        config.stun_servers = stun_server;
    }

    config.external_addresses = external
        .into_iter()
        .map(|addr| addr.parse())
        .collect::<Result<Vec<_>, _>>()
        .context("parse external multiaddr")?;

    let identity = if let Some(path) = identity_path {
        let bytes = fs::read(&path).with_context(|| format!("read identity from {path:?}"))?;
        NodeIdentity::from_keypair_bytes(&bytes)?
    } else {
        NodeIdentity::default()
    };

    if let Some(path) = identity_out {
        let bytes = identity.to_keypair_bytes()?;
        fs::write(&path, &bytes).with_context(|| format!("write identity to {path:?}"))?;
    }

    let (mut node, command_tx) = Node::new(config, identity).await?;
    info!(peer_id = %node.peer_id(), "node started");

    let mut inbound = node.subscribe();
    let mut stdout = tokio::io::stdout();

    let display_task = tokio::spawn(async move {
        while let Ok(msg) = inbound.recv().await {
            let payload = match std::str::from_utf8(&msg.data) {
                Ok(text) => text.trim_end().to_string(),
                Err(_) => hex::encode(&msg.data),
            };
            if let Err(e) = tokio::io::AsyncWriteExt::write_all(
                &mut stdout,
                format!("[{}] {}\n", msg.from, payload).as_bytes(),
            )
            .await
            {
                error!(?e, "failed to write inbound message");
                break;
            }
            if let Err(e) = tokio::io::AsyncWriteExt::flush(&mut stdout).await {
                error!(?e, "failed to flush stdout");
                break;
            }
        }
    });

    if let Some(message) = publish {
        command_tx
            .send(message.into_bytes())
            .map_err(|_| anyhow!("node shut down"))?;
    }

    if !no_stdin {
        let mut reader = BufReader::new(io::stdin());
        let mut line = String::new();
        let tx = command_tx.clone();
        tokio::spawn(async move {
            loop {
                line.clear();
                match reader.read_line(&mut line).await {
                    Ok(0) => break,
                    Ok(_) => {
                        let trimmed = line.trim_end().to_string();
                        if trimmed.is_empty() {
                            continue;
                        }
                        if tx.send(trimmed.into_bytes()).is_err() {
                            break;
                        }
                    }
                    Err(e) => {
                        error!(?e, "stdin read error");
                        break;
                    }
                }
            }
        });
    }

    node.run().await?;
    display_task.abort();
    Ok(())
}
