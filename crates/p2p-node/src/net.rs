use crate::identity::NodeIdentity;
use crate::metrics::NodeMetrics;
use anyhow::{anyhow, Context, Result};
use libp2p::core::{muxing::StreamMuxerBox, transport::Boxed, upgrade::Version};
use libp2p::swarm::NetworkBehaviour;
use libp2p::{
    autonat, dns, gossipsub, identify, kad, noise, ping, quic, tcp, yamux, Multiaddr, PeerId,
    StreamProtocol, Transport,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum NodeError {
    #[error("libp2p error: {0}")]
    Libp2p(String),
    #[error("identity error: {0}")]
    Identity(String),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

#[derive(Clone)]
pub struct NodeConfig {
    pub bootstrap: Vec<Multiaddr>,
    pub local_bind_addr: Multiaddr,
    pub quic_listen_addr: Option<Multiaddr>,
    pub metrics_addr: SocketAddr,
    pub gossip_validation_mode: gossipsub::ValidationMode,
    pub kad_parallelism: u8,
    pub enable_tcp: bool,
    pub enable_quic: bool,
}

impl Default for NodeConfig {
    fn default() -> Self {
        Self {
            bootstrap: Vec::new(),
            local_bind_addr: "/ip4/0.0.0.0/tcp/0".parse().expect("valid addr"),
            quic_listen_addr: Some("/ip4/0.0.0.0/udp/0/quic-v1".parse().expect("valid addr")),
            metrics_addr: "0.0.0.0:9898".parse().expect("valid socket addr"),
            gossip_validation_mode: gossipsub::ValidationMode::Strict,
            kad_parallelism: 3,
            enable_tcp: true,
            enable_quic: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GossipMessage {
    pub from: PeerId,
    pub timestamp: u64,
    pub payload: Vec<u8>,
}

impl GossipMessage {
    pub fn encode(&self) -> Result<Vec<u8>> {
        Ok(serde_json::to_vec(self)?)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self> {
        Ok(serde_json::from_slice(bytes)?)
    }
}

pub fn build_transport(
    identity: &NodeIdentity,
    config: &NodeConfig,
) -> Result<Boxed<(PeerId, StreamMuxerBox)>> {
    let mut transport: Option<Boxed<(PeerId, StreamMuxerBox)>> = None;

    if config.enable_tcp {
        let tcp_transport = tcp::tokio::Transport::new(tcp::Config::default())
            .upgrade(Version::V1Lazy)
            .authenticate(noise::Config::new(identity.keypair())?)
            .multiplex(yamux::Config::default())
            .map(|(peer, muxer), _| (peer, StreamMuxerBox::new(muxer)))
            .boxed();
        transport = Some(tcp_transport);
    }

    if config.enable_quic {
        let quic_transport = quic::tokio::Transport::new(quic::Config::new(identity.keypair()))
            .map(|(peer, muxer), _| (peer, StreamMuxerBox::new(muxer)))
            .boxed();
        transport = Some(match transport {
            Some(existing) => existing.or_transport(quic_transport).boxed(),
            None => quic_transport,
        });
    }

    let transport = transport.ok_or_else(|| anyhow!("no transports enabled"))?;
    let transport = dns::tokio::Transport::system(transport)?;
    Ok(transport.boxed())
}

pub enum NodeEvent {
    Gossipsub(gossipsub::Event),
    Identify(identify::Event),
    Ping(ping::Event),
    Kad(kad::Event),
    Autonat(autonat::Event),
}

impl From<gossipsub::Event> for NodeEvent {
    fn from(event: gossipsub::Event) -> Self {
        Self::Gossipsub(event)
    }
}

impl From<identify::Event> for NodeEvent {
    fn from(event: identify::Event) -> Self {
        Self::Identify(event)
    }
}

impl From<ping::Event> for NodeEvent {
    fn from(event: ping::Event) -> Self {
        Self::Ping(event)
    }
}

impl From<kad::Event> for NodeEvent {
    fn from(event: kad::Event) -> Self {
        Self::Kad(event)
    }
}

impl From<autonat::Event> for NodeEvent {
    fn from(event: autonat::Event) -> Self {
        Self::Autonat(event)
    }
}

#[derive(NetworkBehaviour)]
#[behaviour(to_swarm = "NodeEvent", prelude = "libp2p::swarm::derive_prelude")]
pub struct NodeBehaviour {
    pub gossipsub: gossipsub::Behaviour,
    pub identify: identify::Behaviour,
    pub ping: ping::Behaviour,
    pub kad: kad::Behaviour<kad::store::MemoryStore>,
    pub autonat: autonat::Behaviour,
}

pub fn build_behaviour(
    config: &NodeConfig,
    identity: NodeIdentity,
    _metrics: Arc<NodeMetrics>,
) -> Result<NodeBehaviour> {
    let gossipsub_config = gossipsub::ConfigBuilder::default()
        .heartbeat_interval(Duration::from_secs(1))
        .validation_mode(config.gossip_validation_mode)
        .message_id_fn(|message: &gossipsub::Message| message_id(&message.data))
        .build()
        .context("gossipsub config")?;

    let gossipsub = gossipsub::Behaviour::new(
        gossipsub::MessageAuthenticity::Author(identity.peer_id),
        gossipsub_config,
    )
    .map_err(|e| anyhow!(e))?;

    let identify_config = identify::Config::new("p2p-chatroom/0.1".into(), identity.public_key());
    let ping_config = ping::Config::new().with_interval(Duration::from_secs(30));
    let store = kad::store::MemoryStore::new(identity.peer_id);
    let mut kad_config = kad::Config::new(StreamProtocol::new("/ipfs/kad/1.0.0"));
    kad_config.set_query_timeout(Duration::from_secs(60));
    let kad = kad::Behaviour::with_config(identity.peer_id, store, kad_config);

    let autonat_config = autonat::Config {
        only_global_ips: false,
        ..Default::default()
    };
    let autonat = autonat::Behaviour::new(identity.peer_id, autonat_config);

    Ok(NodeBehaviour {
        gossipsub,
        identify: identify::Behaviour::new(identify_config),
        ping: ping::Behaviour::new(ping_config),
        kad,
        autonat,
    })
}

fn message_id(data: &[u8]) -> gossipsub::MessageId {
    let mut hasher = Sha256::new();
    hasher.update(data);
    gossipsub::MessageId::from(hasher.finalize().to_vec())
}
