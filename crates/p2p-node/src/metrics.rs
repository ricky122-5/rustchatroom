use anyhow::{anyhow, Result};
use hyper::header::{HeaderValue, CONTENT_TYPE};
use hyper::{Body, Response, StatusCode};
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
    pub hole_punch_attempts: IntCounter,
    pub hole_punch_success: IntCounter,
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
        let hole_punch_attempts = IntCounter::with_opts(Opts::new(
            "hole_punch_attempts",
            "Total UDP hole punch attempts initiated",
        ))?;
        let hole_punch_success = IntCounter::with_opts(Opts::new(
            "hole_punch_success",
            "Successful UDP hole punch sessions",
        ))?;

        registry.register(Box::new(gossip_published.clone()))?;
        registry.register(Box::new(gossip_received.clone()))?;
        registry.register(Box::new(peers_gauge.clone()))?;
        registry.register(Box::new(ping_latency.clone()))?;
        registry.register(Box::new(hole_punch_attempts.clone()))?;
        registry.register(Box::new(hole_punch_success.clone()))?;

        Ok(Self {
            registry,
            gossip_published,
            gossip_received,
            peers_gauge,
            ping_latency,
            hole_punch_attempts,
            hole_punch_success,
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
                        if let Err(e) = encoder.encode(&metric_families, &mut buffer) {
                            error!(?e, "encode metrics");
                            let mut response =
                                Response::new(Body::from("failed to encode metrics"));
                            *response.status_mut() = StatusCode::INTERNAL_SERVER_ERROR;
                            return Ok::<_, hyper::Error>(response);
                        }

                        let mut response = Response::new(Body::from(buffer));
                        match HeaderValue::from_str(encoder.format_type()) {
                            Ok(content_type) => {
                                response.headers_mut().insert(CONTENT_TYPE, content_type);
                            }
                            Err(e) => {
                                error!(?e, "invalid content-type header");
                                *response.status_mut() = StatusCode::INTERNAL_SERVER_ERROR;
                                *response.body_mut() = Body::from("invalid content type");
                            }
                        }
                        Ok::<_, hyper::Error>(response)
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
