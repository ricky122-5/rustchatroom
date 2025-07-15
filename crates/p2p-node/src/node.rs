use crate::hole_punch::HolePunchHandle;
use crate::identity::NodeIdentity;
use crate::metrics::NodeMetrics;
use crate::net::{self, GossipMessage, GossipPayload, NodeConfig};
use anyhow::{anyhow, Context, Result};
use futures::StreamExt;
use libp2p::gossipsub;
use libp2p::multiaddr::Protocol;
use libp2p::swarm::{Config, SwarmEvent};
use libp2p::{identify, ping, Multiaddr, Swarm};
use std::collections::HashSet;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{broadcast, mpsc};
use tokio::time::{interval, Duration as IntervalDuration};
use tokio_stream::wrappers::BroadcastStream;
use tracing::{debug, info, warn};

pub const GOSSIPSUB_TOPIC: &str = "rustchatroom";
const ANNOUNCEMENT_INTERVAL: Duration = Duration::from_secs(30);

pub struct Node {
    swarm: Swarm<net::NodeBehaviour>,
    metrics: Arc<NodeMetrics>,
    identity: NodeIdentity,
    _metrics_handle: Option<tokio::task::JoinHandle<()>>,
    gossipsub_topic: gossipsub::IdentTopic,
    inbound_tx: broadcast::Sender<ChatMessage>,
    config: NodeConfig,
    command_rx: mpsc::UnboundedReceiver<Vec<u8>>,
    hole_punch: Option<HolePunchHandle>,
    external_addresses: Vec<Multiaddr>,
    last_announcement: Option<Instant>,
}

#[derive(Clone, Debug)]
pub struct ChatMessage {
    pub from: libp2p::PeerId,
    pub timestamp: u64,
    pub data: Vec<u8>,
}

impl Node {
    pub async fn new(
        config: NodeConfig,
        identity: NodeIdentity,
    ) -> Result<(Self, mpsc::UnboundedSender<Vec<u8>>)> {
        let metrics = Arc::new(NodeMetrics::new()?);
        let transport = net::build_transport(&identity, &config)?;
        let behaviour = net::build_behaviour(&config, identity.clone(), metrics.clone())?;
        let external_addresses = config.external_addresses.clone();
        let hole_punch = if config.enable_udp {
            Some(
                crate::hole_punch::HolePuncher::spawn(
                    metrics.clone(),
                    identity.peer_id,
                    config.udp_port,
                )
                .await?,
            )
        } else {
            None
        };
        let mut swarm = Swarm::new(
            transport,
            behaviour,
            identity.peer_id,
            Config::with_tokio_executor(),
        );

        Swarm::listen_on(&mut swarm, config.local_bind_addr.clone())
            .context("listen on local addr")?;

        if let Some(quic_addr) = &config.quic_listen_addr {
            Swarm::listen_on(&mut swarm, quic_addr.clone()).context("listen on quic addr")?;
        }

        for addr in &config.external_addresses {
            Swarm::add_external_address(&mut swarm, addr.clone());
        }

        if !config.stun_servers.is_empty() {
            if let Ok(discovered) = net::discover_external_addresses(&config.stun_servers).await {
                for addr in discovered {
                    Swarm::add_external_address(&mut swarm, addr);
                }
            }
        }

        for addr in &config.bootstrap {
            let mut addr_no_peer = addr.clone();
            let peer_id = match addr_no_peer.pop() {
                Some(Protocol::P2p(peer)) => Some(peer),
                Some(other) => {
                    addr_no_peer.push(other);
                    None
                }
                None => {
                    warn!(?addr, "bootstrap address missing peer id");
                    None
                }
            };

            if let Some(peer_id) = peer_id {
                swarm
                    .behaviour_mut()
                    .kad
                    .add_address(&peer_id, addr_no_peer.clone());
            }

            if let Err(e) = swarm.dial(addr.clone()) {
                warn!(?addr, ?e, "dial bootstrap failed");
            }
        }

        let topic = gossipsub::IdentTopic::new(GOSSIPSUB_TOPIC);
        swarm
            .behaviour_mut()
            .gossipsub
            .subscribe(&topic)
            .context("subscribe topic")?;

        let (inbound_tx, _) = broadcast::channel::<ChatMessage>(1024);
        let metrics_handle = Some(metrics.clone().spawn_server(config.metrics_addr)?);
        let (command_tx, command_rx) = mpsc::unbounded_channel();

        let mut node = Self {
            swarm,
            metrics,
            identity,
            _metrics_handle: metrics_handle,
            gossipsub_topic: topic,
            inbound_tx,
            config,
            command_rx,
            hole_punch,
            external_addresses,
            last_announcement: None,
        };

        if node.hole_punch.is_some() {
            node.broadcast_announcement()?;
        }

        Ok((node, command_tx))
    }

    pub fn peer_id(&self) -> libp2p::PeerId {
        self.identity.peer_id
    }

    pub fn subscribe(&self) -> broadcast::Receiver<ChatMessage> {
        self.inbound_tx.subscribe()
    }

    pub fn publish(&mut self, payload: Vec<u8>) -> Result<()> {
        self.publish_payload(GossipPayload::Chat { data: payload })?;
        self.metrics.gossip_published.inc();
        Ok(())
    }

    fn publish_payload(&mut self, payload: GossipPayload) -> Result<()> {
        let timestamp = chrono::Utc::now().timestamp_millis() as u64;
        let msg = GossipMessage::new_signed(&self.identity, timestamp, payload)?;
        let data = msg.encode()?;
        self.swarm
            .behaviour_mut()
            .gossipsub
            .publish(self.gossipsub_topic.clone(), data)
            .context("publish message")?;
        Ok(())
    }

    fn broadcast_announcement(&mut self) -> Result<()> {
        if let Some(handle) = &self.hole_punch {
            if self
                .last_announcement
                .map(|t| t.elapsed() < ANNOUNCEMENT_INTERVAL)
                .unwrap_or(false)
            {
                return Ok(());
            }

            let address_strings: Vec<String> = self
                .external_addresses
                .iter()
                .map(|addr| addr.to_string())
                .collect();
            handle.update_local_addresses(self.external_addresses.clone());
            self.publish_payload(GossipPayload::Announcement {
                udp_port: handle.local_port(),
                addresses: address_strings,
            })?;
            self.last_announcement = Some(Instant::now());
        }
        Ok(())
    }

    fn update_external_addresses(&mut self, new_addrs: Vec<Multiaddr>) -> Result<()> {
        let mut existing: HashSet<Multiaddr> = self.external_addresses.iter().cloned().collect();
        let mut changed = false;
        for addr in new_addrs {
            if existing.insert(addr.clone()) {
                self.external_addresses.push(addr);
                changed = true;
            }
        }

        if changed {
            if let Some(handle) = &self.hole_punch {
                handle.update_local_addresses(self.external_addresses.clone());
            }
            self.broadcast_announcement()?;
        }

        Ok(())
    }

    fn handle_announcement(
        &mut self,
        peer_id: libp2p::PeerId,
        udp_port: u16,
        addresses: Vec<String>,
    ) {
        if peer_id == self.identity.peer_id {
            return;
        }

        if let Some(handle) = &self.hole_punch {
            let endpoints: Vec<SocketAddr> = addresses
                .into_iter()
                .filter_map(|addr| string_to_ip(&addr))
                .map(|ip| SocketAddr::new(ip, udp_port))
                .collect();

            if !endpoints.is_empty() {
                handle.punch(peer_id, endpoints);
            }
        }
    }

    pub async fn next_message(&self) -> Result<ChatMessage> {
        let mut stream = BroadcastStream::new(self.inbound_tx.subscribe());
        match stream.next().await {
            Some(Ok(msg)) => Ok(msg),
            Some(Err(e)) => Err(e.into()),
            None => Err(anyhow!("stream closed")),
        }
    }

    pub async fn run(&mut self) -> Result<()> {
        let stun_enabled = !self.config.stun_servers.is_empty();
        let mut stun_interval = interval(IntervalDuration::from_secs(120));

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
                Some(command) = self.command_rx.recv() => {
                    if let Err(e) = self.publish(command) {
                        warn!(?e, "failed to publish command message");
                    }
                }
                _ = stun_interval.tick(), if stun_enabled => {
                    if let Ok(addrs) = net::discover_external_addresses(&self.config.stun_servers).await {
                        for addr in &addrs {
                            Swarm::add_external_address(&mut self.swarm, addr.clone());
                        }
                        self.update_external_addresses(addrs)?;
                    }
                }
            }
        }
    }

    async fn handle_event(&mut self, event: net::NodeEvent) -> Result<()> {
        match event {
            net::NodeEvent::Gossipsub(event) => match event {
                gossipsub::Event::Message {
                    propagation_source,
                    message_id,
                    message,
                } => {
                    self.metrics.gossip_received.inc();
                    debug!(?propagation_source, ?message_id, "gossipsub message");
                    if let Ok(msg) = GossipMessage::decode(&message.data) {
                        match msg.verify() {
                            Ok(()) => match msg.payload {
                                GossipPayload::Chat { data } => {
                                    let chat = ChatMessage {
                                        from: msg.from,
                                        timestamp: msg.timestamp,
                                        data,
                                    };
                                    let _ = self.inbound_tx.send(chat);
                                }
                                GossipPayload::Announcement {
                                    udp_port,
                                    addresses,
                                } => {
                                    self.handle_announcement(msg.from, udp_port, addresses);
                                }
                            },
                            Err(e) => {
                                warn!(
                                    ?propagation_source,
                                    ?message_id,
                                    ?e,
                                    "discarding unverifiable message"
                                );
                            }
                        }
                    }
                }
                gossipsub::Event::Subscribed { peer_id, topic } => {
                    debug!(?peer_id, ?topic, "peer subscribed");
                }
                gossipsub::Event::Unsubscribed { peer_id, topic } => {
                    debug!(?peer_id, ?topic, "peer unsubscribed");
                }
                other => {
                    debug!(?other, "gossipsub event");
                }
            },
            net::NodeEvent::Identify(event) => match event {
                identify::Event::Received { peer_id, info, .. } => {
                    debug!(?peer_id, protocols = ?info.protocols, "identify info");
                }
                identify::Event::Sent { peer_id, .. } => {
                    debug!(?peer_id, "identify info sent");
                }
                identify::Event::Pushed { peer_id, info, .. } => {
                    debug!(?peer_id, protocols = ?info.protocols, "identify info pushed");
                }
                identify::Event::Error { peer_id, error, .. } => {
                    warn!(?peer_id, ?error, "identify error");
                }
            },
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

fn string_to_ip(addr: &str) -> Option<IpAddr> {
    let multiaddr: Multiaddr = addr.parse().ok()?;
    let mut iter = multiaddr.iter();
    match iter.next()? {
        Protocol::Ip4(ip) => Some(IpAddr::V4(ip)),
        Protocol::Ip6(ip) => Some(IpAddr::V6(ip)),
        _ => None,
    }
}

pub type NodeEvent = <net::NodeBehaviour as libp2p::swarm::NetworkBehaviour>::ToSwarm;
