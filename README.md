# Rust P2P Chatroom Research Prototype

This workspace hosts a modular libp2p research node and CLI that explores Ethereum-aligned peer-to-peer patterns (Noise authenticated transports, Kademlia DHT, GossipSub-based chat, Prometheus metrics, and NAT traversal helpers). It is designed both as a sandbox for protocol research and as a reproducible experiment harness.

## Highlights

- **Identity / Security** – secp256k1-backed libp2p identities with signed gossip payloads and Noise-XX handshakes.
- **Multi-transport networking** – configurable TCP / UDP / QUIC transports, DNS support, and automatic bootstrap dialing via Kademlia.
- **NAT traversal helpers** – STUN discovery loop publishes external multiaddrs and coordinates UDP hole punching for peers behind home NATs.
- **Pubsub chat** – GossipSub with strict validation, SHA-256 message IDs, and signed payloads surfaced to downstream clients.
- **Observability** – Prometheus metrics server (latency, gossip in/out, peer counts) and hooks for external dashboards.
- **Automation** – Docker + k3d manifests and netem-ready scripts for repeating large cluster experiments.

## Quick Start

```bash
cargo install --path crates/p2p-cli
p2p-chat run \
  --listen /ip4/0.0.0.0/tcp/9000 \
  --metrics 0.0.0.0:9898 \
  --bootstrap /ip4/203.0.113.10/tcp/30333/p2p/12D3KooWSamplePeer
```

The CLI exposes:

- `--quic-only` / `--tcp-only` toggles
- `--identity PATH` to load a protobuf-encoded keypair
- `--identity-out PATH` to persist the generated secp256k1 identity
- `--publish` to broadcast a single payload on startup
- interactive stdin message loop (disable with `--no-stdin`)
- `--stun-server host:port` overrides (defaults to Google + Twilio STUN)
- `--disable-udp` / `--udp-port` to control the UDP hole-punch service
- multiaddrs passed via `--external` are published immediately

Prometheus metrics are served at the chosen `--metrics` address. Example scrape configuration is available in `deploy/observability/prometheus.yml`.

Sample raw traces and generated plots from a 67-node simulation are published under `docs/artifacts/`.

## Repository Layout

| Path              | Description                                                                 |
| ----------------- | --------------------------------------------------------------------------- |
| `crates/p2p-node` | Core networking library (transport, identity, DHT, GossipSub, metrics).     |
| `crates/p2p-cli`  | Tokio CLI wrapping the node (bootstrap, stdin REPL, metrics wiring).        |
| `deploy/docker`   | Dockerfile & compose targets for local clusters.                            |
| `deploy/k3d`      | k3d / Kubernetes manifests and orchestration helpers.                       |
| `docs/`           | Experiment recipes, Prometheus dashboards, Grafana JSON, research write-up. |
| `scripts/`        | Automation helpers for netem, experiment runs, and metrics export.          |

## Experimentation

The `scripts/` directory contains:

- `launch-k3d.sh` – creates a 7-node k3d cluster, deploys Prometheus/Grafana, and applies chatroom manifests.
- `run-experiment.sh` – parameterised wrapper for running N chat nodes with configurable delay/loss via `tc` + netem.
- `collect-metrics.py` – scrapes Prometheus endpoints and aggregates gossip throughput + RTT stats into CSV.
- `generate-plots.py` – converts the CSV trace into PNG plots (gossip throughput, peers, latency, hole-punch stats).
- `generate-plots.py` – turns the CSV trace into PNG throughput/latency plots for reports.

See `docs/experiments.md` for end-to-end instructions (building Docker images, seeding bootstrap peers, running a 67-node experiment, and generating Grafana dashboards).

## Development

```bash
cargo fmt
cargo clippy --all-targets
cargo test

# Run a local node with logging
RUST_LOG=info cargo run -p p2p-cli -- run --metrics 127.0.0.1:9898

# Inspect metrics
curl -s localhost:9898/metrics
```

## Future Work

- Promote the UDP hole-punch daemon into a reusable libp2p behaviour (relay/DCUtR integration).
- Expand request/response protocols for richer messaging beyond chat payloads.
- Add property-based tests for GossipSub message validation.
- Automate publication of experiment artefacts (plots + raw traces) via GitHub Releases.
