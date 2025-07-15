use crate::metrics::NodeMetrics;
use anyhow::{Context, Result};
use libp2p::PeerId;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::net::UdpSocket;
use tokio::sync::{mpsc, RwLock};
use tokio::time::sleep;
use tracing::{debug, info, warn};

#[derive(Clone)]
pub struct HolePunchHandle {
    tx: mpsc::UnboundedSender<Command>,
    local_port: u16,
}

impl HolePunchHandle {
    pub fn local_port(&self) -> u16 {
        self.local_port
    }

    pub fn update_local_addresses(&self, addresses: Vec<libp2p::Multiaddr>) {
        let _ = self.tx.send(Command::UpdateLocal {
            addresses: addresses.into_iter().map(|addr| addr.to_string()).collect(),
        });
    }

    pub fn punch(&self, peer_id: PeerId, endpoints: Vec<SocketAddr>) {
        if endpoints.is_empty() {
            return;
        }
        let _ = self.tx.send(Command::Initiate { peer_id, endpoints });
    }
}

pub struct HolePuncher;

impl HolePuncher {
    pub async fn spawn(
        metrics: Arc<NodeMetrics>,
        peer_id: PeerId,
        port: u16,
    ) -> Result<HolePunchHandle> {
        let socket = UdpSocket::bind(("0.0.0.0", port))
            .await
            .context("bind udp socket for hole punch")?;
        socket
            .set_broadcast(true)
            .context("enable broadcast on udp socket")?;
        let local_port = socket.local_addr()?.port();
        let socket = Arc::new(socket);
        let (tx, mut rx) = mpsc::unbounded_channel();
        let local_addresses = Arc::new(RwLock::new(Vec::<String>::new()));
        let metrics_send = metrics.clone();
        let send_socket = socket.clone();
        let local_addr_store = local_addresses.clone();
        let local_peer_id = peer_id.to_string();

        tokio::spawn(async move {
            while let Some(cmd) = rx.recv().await {
                match cmd {
                    Command::UpdateLocal { addresses } => {
                        *local_addr_store.write().await = addresses;
                    }
                    Command::Initiate { peer_id, endpoints } => {
                        metrics_send.hole_punch_attempts.inc();

                        let addresses = local_addr_store.read().await.clone();
                        let packet = PunchPacket {
                            peer_id: local_peer_id.clone(),
                            addresses,
                            timestamp: current_millis(),
                        };

                        match serde_json::to_vec(&packet) {
                            Ok(bytes) => {
                                for attempt in 0..3 {
                                    for endpoint in &endpoints {
                                        if let Err(e) = send_socket.send_to(&bytes, endpoint).await
                                        {
                                            warn!(?endpoint, ?e, "hole punch send error");
                                        } else {
                                            debug!(?endpoint, target = ?peer_id, attempt, "sent hole punch packet");
                                        }
                                    }
                                    sleep(Duration::from_millis(250)).await;
                                }
                            }
                            Err(e) => warn!(?e, "serialize hole punch packet"),
                        }
                    }
                }
            }
        });

        let recv_socket = socket.clone();
        let recv_addresses = local_addresses.clone();
        let metrics_recv = metrics;
        let local_peer_id_recv = peer_id.to_string();

        tokio::spawn(async move {
            let mut buf = vec![0u8; 2048];
            loop {
                match recv_socket.recv_from(&mut buf).await {
                    Ok((len, addr)) => {
                        if let Ok(packet) = serde_json::from_slice::<PunchPacket>(&buf[..len]) {
                            metrics_recv.hole_punch_success.inc();
                            info!(?addr, from = %packet.peer_id, "received hole punch datagram");

                            let reply = PunchPacket {
                                peer_id: local_peer_id_recv.clone(),
                                addresses: recv_addresses.read().await.clone(),
                                timestamp: current_millis(),
                            };

                            if let Ok(bytes) = serde_json::to_vec(&reply) {
                                if let Err(e) = recv_socket.send_to(&bytes, addr).await {
                                    warn!(?addr, ?e, "hole punch ack send error");
                                }
                            }
                        } else {
                            debug!(?addr, "received unknown udp packet");
                        }
                    }
                    Err(e) => {
                        warn!(?e, "hole punch receive error");
                    }
                }
            }
        });

        Ok(HolePunchHandle { tx, local_port })
    }
}

enum Command {
    UpdateLocal {
        addresses: Vec<String>,
    },
    Initiate {
        peer_id: PeerId,
        endpoints: Vec<SocketAddr>,
    },
}

#[derive(serde::Serialize, serde::Deserialize)]
struct PunchPacket {
    peer_id: String,
    addresses: Vec<String>,
    timestamp: u64,
}

fn current_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_else(|_| Duration::from_secs(0))
        .as_millis() as u64
}
