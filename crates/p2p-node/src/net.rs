use crate::identity::NodeIdentity;
use crate::metrics::NodeMetrics;
use crate::node::{GossipMessage, NodeEvent};
use anyhow::{Context, Result};
use libp2p::swarm::NetworkBehaviour;
use libp2p::TransportExt;
use libp2p::{autonat, gossipsub, identify, kad, ping, request_response, SwarmBuilder};
use serde::{Deserialize, Serialize};
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
    pub bootstrap: Vec<String>,
    pub stun_servers: Vec<String>,
    pub listen_tcp: String,
    pub listen_quic: Option<String>,
    pub metrics_addr: SocketAddr,
    pub gossip_validation_mode: gossipsub::ValidationMode,
    pub kad_parallelism: u8,
}

impl Default for NodeConfig {
    fn default() -> Self {
        Self {
            bootstrap: Vec::new(),
            stun_servers: vec!["stun.l.google.com:19302".into()],
            listen_tcp: "/ip4/0.0.0.0/tcp/0".into(),
            listen_quic: Some("/ip4/0.0.0.0/udp/0/quic-v1".into()),
            metrics_addr: "0.0.0.0:9898".parse().expect("valid socket addr"),
            gossip_validation_mode: gossipsub::ValidationMode::Strict,
            kad_parallelism: 3,
        }
    }
}

pub fn build_swarm(
    config: &NodeConfig,
    identity: &NodeIdentity,
) -> Result<libp2p::Swarm<NodeBehaviour>> {
    let mut builder = SwarmBuilder::with_existing_identity(identity.keypair().clone()).with_tokio();

    builder = builder.with_tcp(
        Default::default(),
        (libp2p::tls::Config::new, libp2p::noise::Config::new),
        libp2p::yamux::Config::default,
    )?;

    if config.listen_quic.is_some() {
        builder = builder.with_quic();
    }

    builder = builder.with_behaviour(|_| {
        build_behaviour(config, identity.clone(), Arc::new(NodeMetrics::new()?))
    })?;

    let mut swarm = builder.build();

    let listen_tcp = config.listen_tcp.parse()?;
    swarm.listen_on(listen_tcp)?;

    if let Some(quic_addr) = &config.listen_quic {
        swarm.listen_on(quic_addr.parse()?)?;
    }

    for addr in &config.bootstrap {
        if let Ok(multiaddr) = addr.parse() {
            swarm
                .behaviour_mut()
                .kad
                .add_address(&identity.peer_id, multiaddr);
        }
    }

    Ok(swarm)
}

#[derive(NetworkBehaviour)]
#[behaviour(to_swarm = "NodeEvent", prelude = "libp2p::swarm::derive_prelude")]
pub struct NodeBehaviour {
    pub gossipsub: gossipsub::Behaviour,
    pub identify: identify::Behaviour,
    pub ping: ping::Behaviour,
    pub kad: kad::Behaviour<kad::store::MemoryStore>,
    pub autonat: autonat::Behaviour,
    pub request_response: request_response::Behaviour<ChatCodec>,
}

pub fn build_behaviour(
    config: &NodeConfig,
    identity: NodeIdentity,
    _metrics: Arc<NodeMetrics>,
) -> Result<NodeBehaviour> {
    let gossipsub_config = gossipsub::ConfigBuilder::default()
        .heartbeat_interval(Duration::from_secs(1))
        .validation_mode(config.gossip_validation_mode)
        .message_id_fn(|message: &gossipsub::Message| {
            gossipsub::MessageId::from(super::node::hash_message(&message.data))
        })
        .build()
        .context("gossipsub config")?;

    let gossipsub = gossipsub::Behaviour::new(
        gossipsub::MessageAuthenticity::Author(identity.peer_id),
        gossipsub_config,
    )?;

    let identify_config = identify::Config::new("p2p-chatroom/0.1".into(), identity.public_key());
    let ping_config = ping::Config::new().with_interval(Duration::from_secs(30));
    let store = kad::store::MemoryStore::new(identity.peer_id);
    let mut kad = kad::Behaviour::with_config(identity.peer_id, store, kad::Config::default());
    kad.set_query_parallelism(config.kad_parallelism.into());

    let autonat_config = autonat::Config {
        only_global_ips: false,
        ..Default::default()
    };
    let autonat = autonat::Behaviour::new(identity.peer_id, autonat_config);

    let protocols = std::iter::once(ChatProtocol);
    let request_response =
        request_response::Behaviour::new(ChatCodec::default(), protocols, Default::default());

    Ok(NodeBehaviour {
        gossipsub,
        identify: identify::Behaviour::new(identify_config),
        ping: ping::Behaviour::new(ping_config),
        kad,
        autonat,
        request_response,
    })
}
