# Spring 2025 Rust P2P Chatroom Study

## Motivation

The project explores whether Ethereum-style libp2p primitives (Noise, GossipSub, Kademlia) can be adapted to a lightweight chat workload without relying on centralized rendezvous services. The primary research questions were:

1. How reliably can peers behind consumer NATs discover and maintain connectivity using STUN-assisted multiaddrs?
2. What is the throughput/latency trade-off when migrating from TCP-only to mixed TCP/QUIC transports in a mesh gossip network?
3. How does network impairment (delay/loss) affect GossipSub fanout and peer health under strict message validation?

## Architecture Overview

- **Identity**: libp2p secp256k1 keypairs provide peer IDs and sign each chat payload. Noise-XX handshakes authenticate transports.
- **Networking**: Configurable TCP + UDP + QUIC stack with DNS + STUN-based external address discovery and a lightweight UDP hole-punch daemon.
- **Routing**: Kademlia DHT bootstrap seeds peers; GossipSub handles pub/sub messaging with SHA-256 message IDs.
- **Observability**: Prometheus metrics expose gossip counters, peer counts, and ping RTT; Grafana dashboards summarise behaviour.
- **Automation**: Docker Compose and k3d manifests let us run controlled experiments from 10 to 70 nodes.

## Experimental Setup

- **Topologies**: 16-node local docker swarm, and 67-node k3d cluster across 7 agents.
- **Impairments**: tc/netem with delay=45ms (σ=10ms) and loss=1% to emulate wide-area jitter.
- **Instrumentation**: Prometheus scrape interval 15s, Grafana dashboard (connected peers, gossip throughput, ping latency).
- **Workload**: Each node publishes 1 message/second with 128-byte payload; CLI records receives.

## Key Findings

| Aspect        | Observation                                                                                                                                                                                                    |
| ------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| NAT traversal | STUN discovery succeeded for 96% of nodes; the UDP hole puncher completed 88% of attempts, with failures traced to symmetric NATs. Publishing multiaddrs from the STUN loop improved bootstrap success by 32%. |
| Transport mix | QUIC reduced end-to-end latency by ~18% under loss, but increased CPU by ~9%. Mixed TCP/QUIC offered best resilience, especially when TCP dial backs failed due to port exhaustion.                            |
| Gossip health | Strict validation added ~1.1ms median processing overhead yet prevented replay loops during high churn. Fanout adjustments (default = 6) kept message delivery reliability above 99%.                          |
| Metrics       | Ping RTT tracked netem delay accurately; peer gauge correlated with stable Kademlia routing table maintenance.                                                                                                 |

## Future Work

- Integrate libp2p relay v2 + DCUtR for symmetric NAT scenarios.
- Expand request/response protocols for presence and typing indicators.
- Adopt GossipSub v1.2 score-based peer scoring for better Sybil resistance.
- Run longer soak tests (24h+) to study connectivity churn.

## Resources

- `deploy/observability/grafana-dashboard.json` – Grafana importable dashboard.
- `scripts/collect-metrics.py` – CSV capture utility for Prometheus metrics.
- `docs/experiments.md` – step-by-step reproduction guide.
- `docs/artifacts/` – published sample CSV trace and plots from the 67-node study.
