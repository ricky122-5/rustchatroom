use crate::identity::NodeIdentity;
use crate::metrics::NodeMetrics;
use crate::net::{self, GossipMessage, NodeConfig};
use anyhow::{anyhow, Context, Result};
use futures::StreamExt;
use libp2p::gossipsub;
use libp2p::swarm::SwarmEvent;
use libp2p::{identify, ping, Swarm};
use std::sync::Arc;
use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;
use tracing::{debug, info, warn};

pub const GOSSIPSUB_TOPIC: &str = "rustchatroom";

fn hash_message(data: &[u8]) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(data);
    hasher.finalize().into()
}

pub struct Node {
    swarm: Swarm<net::NodeBehaviour>,
    metrics: Arc<NodeMetrics>,
    identity: NodeIdentity,
    metrics_handle: Option<tokio::task::JoinHandle<()>>,
    gossipsub_topic: gossipsub::IdentTopic,
    inbound_tx: broadcast::Sender<GossipMessage>,
}

impl Node {
    pub async fn new(config: NodeConfig, identity: NodeIdentity) -> Result<Self> {
        let metrics = Arc::new(NodeMetrics::new()?);
        let transport = net::build_transport(&identity, &config)?;
        let behaviour = net::build_behaviour(&config, identity.clone(), metrics.clone())?;
        let mut swarm = Swarm::new(transport, behaviour, identity.peer_id);

        Swarm::listen_on(&mut swarm, config.local_bind_addr.clone())
            .context("listen on local addr")?;

        if let Some(quic_addr) = &config.quic_listen_addr {
            Swarm::listen_on(&mut swarm, quic_addr.clone()).context("listen on quic addr")?;
        }

        for addr in &config.bootstrap {
            swarm
                .behaviour_mut()
                .kad
                .add_address(&identity.peer_id, addr.clone());
        }

        let topic = gossipsub::IdentTopic::new(GOSSIPSUB_TOPIC);
        swarm
            .behaviour_mut()
            .gossipsub
            .subscribe(&topic)
            .context("subscribe topic")?;

        let inbound_tx = broadcast::channel(1024).0;
        let metrics_handle = Some(metrics.clone().spawn_server(config.metrics_addr)?);

        Ok(Self {
            swarm,
            metrics,
            identity,
            metrics_handle,
            gossipsub_topic: topic,
            inbound_tx,
        })
    }

    pub fn peer_id(&self) -> libp2p::PeerId {
        self.identity.peer_id
    }

    pub fn publish(&mut self, payload: Vec<u8>) -> Result<()> {
        let msg = GossipMessage {
            from: self.identity.peer_id,
            timestamp: chrono::Utc::now().timestamp_millis() as u64,
            payload,
        };
        let data = msg.encode()?;
        self.swarm
            .behaviour_mut()
            .gossipsub
            .publish(self.gossipsub_topic.clone(), data)
            .context("publish message")?;
        self.metrics.gossip_published.inc();
        Ok(())
    }

    pub async fn next_message(&self) -> Result<GossipMessage> {
        let mut stream = BroadcastStream::new(self.inbound_tx.subscribe());
        match stream.next().await {
            Some(Ok(msg)) => Ok(msg),
            Some(Err(e)) => Err(anyhow!(e)),
            None => Err(anyhow!("stream closed")),
        }
    }

    pub async fn run(mut self) -> Result<()> {
        loop {
            tokio::select! {
                event = self.swarm.next() => {
                    match event {
                        Some(SwarmEvent::Behaviour(event)) => {
                            self.handle_event(event).await?;
                        }
                        Some(SwarmEvent::ConnectionEstablished { peer_id, .. }) => {
                            info!(?peer_id, "connection established");
                            self.metrics.peers_gauge.inc();
                        }
                        Some(SwarmEvent::ConnectionClosed { peer_id, .. }) => {
                            info!(?peer_id, "connection closed");
                            self.metrics.peers_gauge.dec();
                        }
                        Some(SwarmEvent::NewListenAddr { address, .. }) => {
                            info!(?address, "listening");
                        }
                        Some(SwarmEvent::OutgoingConnectionError { peer_id, error, .. }) => {
                            warn!(?peer_id, ?error, "outgoing connection error");
                        }
                        Some(SwarmEvent::IncomingConnectionError { send_back_addr, error, .. }) => {
                            warn!(?send_back_addr, ?error, "incoming connection error");
                        }
                        None => {
                            return Err(anyhow!("swarm terminated"));
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    async fn handle_event(&mut self, event: net::NodeEvent) -> Result<()> {
        match event {
            net::NodeEvent::Gossipsub(gossipsub::Event::Message {
                propagation_source,
                message_id,
                message,
            }) => {
                self.metrics.gossip_received.inc();
                debug!(?propagation_source, ?message_id, "gossipsub message");
                if let Ok(msg) = GossipMessage::decode(&message.data) {
                    let _ = self.inbound_tx.send(msg);
                }
            }
            net::NodeEvent::Gossipsub(gossipsub::Event::Subscribed { peer_id, topic }) => {
                debug!(?peer_id, ?topic, "peer subscribed");
            }
            net::NodeEvent::Gossipsub(gossipsub::Event::Unsubscribed { peer_id, topic }) => {
                debug!(?peer_id, ?topic, "peer unsubscribed");
            }
            net::NodeEvent::Identify(identify::Event::Received { peer_id, info }) => {
                debug!(?peer_id, protocols = ?info.protocols, "identify info");
            }
            net::NodeEvent::Ping(ping::Event { peer, result, .. }) => match result {
                Ok(rtt) => self.metrics.ping_latency.set(rtt.as_millis() as i64),
                Err(e) => warn!(?peer, ?e, "ping failed"),
            },
            net::NodeEvent::Kad(event) => {
                debug!(?event, "kad event");
            }
            net::NodeEvent::Autonat(event) => {
                debug!(?event, "autonat event");
            }
        }
        Ok(())
    }
}

pub type NodeEvent = <net::NodeBehaviour as libp2p::swarm::NetworkBehaviour>::ToSwarm;
