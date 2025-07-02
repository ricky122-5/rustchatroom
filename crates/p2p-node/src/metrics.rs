use anyhow::{anyhow, Result};
use prometheus::{Encoder, IntCounter, IntGauge, Opts, Registry, TextEncoder};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::task::JoinHandle;
use tracing::error;

const PROMETHEUS_PREFIX: &str = "p2p_node";

pub struct NodeMetrics {
    pub registry: Registry,
    pub gossip_published: IntCounter,
    pub gossip_received: IntCounter,
    pub peers_gauge: IntGauge,
    pub ping_latency: IntGauge,
}

impl NodeMetrics {
    pub fn new() -> Result<Self> {
        let registry = Registry::new_custom(Some(PROMETHEUS_PREFIX.into()), None)?;

        let gossip_published = IntCounter::with_opts(Opts::new(
            "gossip_published",
            "Total gossip messages published",
        ))?;
        let gossip_received = IntCounter::with_opts(Opts::new(
            "gossip_received",
            "Total gossip messages received",
        ))?;
        let peers_gauge = IntGauge::with_opts(Opts::new("peers", "Current connected peers"))?;
        let ping_latency = IntGauge::with_opts(Opts::new(
            "ping_latency_ms",
            "Last observed RTT to peer in milliseconds",
        ))?;

        registry.register(Box::new(gossip_published.clone()))?;
        registry.register(Box::new(gossip_received.clone()))?;
        registry.register(Box::new(peers_gauge.clone()))?;
        registry.register(Box::new(ping_latency.clone()))?;

        Ok(Self {
            registry,
            gossip_published,
            gossip_received,
            peers_gauge,
            ping_latency,
        })
    }

    pub fn spawn_server(self: Arc<Self>, addr: SocketAddr) -> Result<JoinHandle<()>> {
        let server = hyper::Server::try_bind(&addr).map_err(|e| anyhow!("metrics bind: {e}"))?;
        let registry = self.registry.clone();

        let service = hyper::service::make_service_fn(move |_| {
            let registry = registry.clone();
            async move {
                Ok::<_, hyper::Error>(hyper::service::service_fn(move |_req| {
                    let registry = registry.clone();
                    async move {
                        let encoder = TextEncoder::new();
                        let metric_families = registry.gather();
                        let mut buffer = Vec::new();
                        encoder.encode(&metric_families, &mut buffer).map_err(|e| {
                            error!(?e, "encode metrics");
                            hyper::Error::new_body_write_aborted()
                        })?;

                        Ok::<_, hyper::Error>(
                            hyper::Response::builder()
                                .status(hyper::StatusCode::OK)
                                .header(hyper::header::CONTENT_TYPE, encoder.format_type())
                                .body(hyper::Body::from(buffer))?,
                        )
                    }
                }))
            }
        });

        let handle = tokio::spawn(async move {
            if let Err(e) = server.serve(service).await {
                error!(?e, "metrics server error");
            }
        });
        Ok(handle)
    }
}
