use anyhow::Result;
use clap::{Parser, Subcommand};
use once_cell::sync::Lazy;
use p2p_node::{identity::NodeIdentity, net::NodeConfig, Node};
use rand::SeedableRng;
use std::net::SocketAddr;
use tracing::info;
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
        } => {
            run_node(
                listen,
                metrics,
                bootstrap,
                validation,
                kad_parallelism,
                quic_only,
                tcp_only,
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
    config.quic = quic_only || (!quic_only && !tcp_only);
    config.tcp = tcp_only || (!quic_only && !tcp_only);

    let identity = NodeIdentity::default();

    let mut node = Node::new(config, identity).await?;
    info!(peer_id = %node.peer_id(), "node started");
    node.run().await
}
