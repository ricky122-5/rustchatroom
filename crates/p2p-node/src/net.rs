use crate::identity::NodeIdentity;
use crate::metrics::NodeMetrics;
use anyhow::{anyhow, Context, Result};
use futures::future::Either;
use libp2p::core::{muxing::StreamMuxerBox, transport::Boxed, upgrade::Version};
use libp2p::multiaddr::Protocol;
use libp2p::swarm::NetworkBehaviour;
use libp2p::{
    autonat, dns, gossipsub, identify, kad, noise, ping, quic, tcp, yamux, Multiaddr, PeerId,
    StreamProtocol, Transport,
};
use libp2p_identity as identity;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::net::{IpAddr, SocketAddr, UdpSocket};
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::time::Duration;
use stunclient::StunClient;
use thiserror::Error;
use tokio::task;
use tracing::warn;

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
    pub enable_udp: bool,
    pub udp_port: u16,
    pub stun_servers: Vec<SocketAddr>,
    pub external_addresses: Vec<Multiaddr>,
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
            enable_udp: true,
            udp_port: 0,
            stun_servers: vec![
                "34.120.84.149:3478".parse().expect("valid stun addr"), // stun.l.google.com
                "34.192.213.134:3478".parse().expect("valid stun addr"), // global.stun.twilio.com
            ],
            external_addresses: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GossipMessage {
    pub from: PeerId,
    pub timestamp: u64,
    pub payload: GossipPayload,
    pub signature: Vec<u8>,
    pub public_key: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum GossipPayload {
    Chat {
        data: Vec<u8>,
    },
    Announcement {
        udp_port: u16,
        addresses: Vec<String>,
    },
}

impl GossipMessage {
    pub fn new_signed(
        identity: &NodeIdentity,
        timestamp: u64,
        payload: GossipPayload,
    ) -> Result<Self> {
        let payload_bytes = serde_json::to_vec(&payload)?;
        let material = signature_material(&identity.peer_id, timestamp, &payload_bytes);
        let signature = identity.sign(&material)?;
        Ok(Self {
            from: identity.peer_id,
            timestamp,
            payload,
            signature,
            public_key: identity.public_key_protobuf(),
        })
    }

    pub fn chat(identity: &NodeIdentity, timestamp: u64, data: Vec<u8>) -> Result<Self> {
        Self::new_signed(identity, timestamp, GossipPayload::Chat { data })
    }

    pub fn announcement(
        identity: &NodeIdentity,
        timestamp: u64,
        udp_port: u16,
        addresses: Vec<String>,
    ) -> Result<Self> {
        Self::new_signed(
            identity,
            timestamp,
            GossipPayload::Announcement {
                udp_port,
                addresses,
            },
        )
    }

    pub fn encode(&self) -> Result<Vec<u8>> {
        Ok(serde_json::to_vec(self)?)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self> {
        Ok(serde_json::from_slice(bytes)?)
    }

    pub fn verify(&self) -> Result<()> {
        let libp2p_key = identity::PublicKey::try_decode_protobuf(&self.public_key)
            .map_err(|e| anyhow!("decode public key: {e}"))?;

        let derived_peer = PeerId::from(libp2p_key.clone());
        if derived_peer != self.from {
            return Err(anyhow!(
                "message public key peer id mismatch: expected {from:?}, got {derived:?}",
                from = self.from,
                derived = derived_peer
            ));
        }

        let payload_bytes = serde_json::to_vec(&self.payload)?;
        let material = signature_material(&self.from, self.timestamp, &payload_bytes);
        if libp2p_key.verify(&material, &self.signature) {
            Ok(())
        } else {
            Err(anyhow!("signature verification failed"))
        }
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
            Some(existing) => existing
                .or_transport(quic_transport)
                .map(|either, _| match either {
                    Either::Left((peer, muxer)) => (peer, muxer),
                    Either::Right((peer, muxer)) => (peer, muxer),
                })
                .boxed(),
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
        .validation_mode(config.gossip_validation_mode.clone())
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
    if let Some(parallelism) = NonZeroUsize::new(config.kad_parallelism.max(1) as usize) {
        kad_config.set_parallelism(parallelism);
    }
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

fn signature_material(peer: &PeerId, timestamp: u64, payload: &[u8]) -> Vec<u8> {
    let mut data = Vec::with_capacity(peer.clone().to_bytes().len() + payload.len() + 8);
    data.extend_from_slice(&peer.clone().to_bytes());
    data.extend_from_slice(&timestamp.to_le_bytes());
    data.extend_from_slice(payload);
    data
}

pub async fn discover_external_addresses(stun_servers: &[SocketAddr]) -> Result<Vec<Multiaddr>> {
    if stun_servers.is_empty() {
        return Ok(Vec::new());
    }

    let mut discovered = HashSet::new();

    for server in stun_servers {
        let server_addr = *server;
        match task::spawn_blocking(move || query_stun(server_addr)).await {
            Ok(Ok(addr)) => {
                for multi in socketaddr_to_multiaddrs(addr) {
                    discovered.insert(multi);
                }
            }
            Ok(Err(e)) => {
                warn!(?server, ?e, "stun discovery failed");
            }
            Err(e) => {
                warn!(?server, ?e, "stun task join error");
            }
        }
    }

    Ok(discovered.into_iter().collect())
}

fn query_stun(server: SocketAddr) -> Result<SocketAddr> {
    let socket = UdpSocket::bind("0.0.0.0:0")?;
    socket.connect(server)?;
    socket.set_read_timeout(Some(Duration::from_secs(3)))?;
    socket.set_write_timeout(Some(Duration::from_secs(3)))?;
    let client = StunClient::new(server);
    let addr = client.query_external_address(&socket)?;
    Ok(addr)
}

fn socketaddr_to_multiaddrs(addr: SocketAddr) -> Vec<Multiaddr> {
    let mut addrs = Vec::new();
    let ip_proto = match addr.ip() {
        IpAddr::V4(v4) => Protocol::Ip4(v4),
        IpAddr::V6(v6) => Protocol::Ip6(v6),
    };

    let mut udp_addr = Multiaddr::empty();
    udp_addr.push(ip_proto.clone());
    udp_addr.push(Protocol::Udp(addr.port()));
    addrs.push(udp_addr);

    let mut tcp_addr = Multiaddr::empty();
    tcp_addr.push(ip_proto);
    tcp_addr.push(Protocol::Tcp(addr.port()));
    addrs.push(tcp_addr);

    addrs
}
