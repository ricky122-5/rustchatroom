# Experiment Playbook

This document captures the procedure used to reproduce the Spring 2025 67-node evaluation. The workflow is designed to be idempotent so you can iterate on protocol tweaks and quickly gather comparable metrics.

## Prerequisites

- Docker Engine 24+
- k3d 5.6+
- kubectl 1.29+
- Helm 3.14+
- Python 3.10+
- Prometheus-compatible storage (optional, default is in-cluster)

## Local Docker Experiment (up to ~20 nodes)

1. **Build + bootstrap**

   ```bash
   export BOOTSTRAP_MULTIADDR="/dns4/bootstrap/tcp/4001/p2p/12D3KooWbootstrap"
   docker compose -f deploy/docker/docker-compose.yml up -d --build
   ```

2. **Scale chat nodes**

   ```bash
   docker compose -f deploy/docker/docker-compose.yml up -d --scale chat=12
   ```

3. **Inject delay/loss (optional)**

   ```bash
   for c in $(docker compose -f deploy/docker/docker-compose.yml ps -q chat); do
     docker exec "$c" tc qdisc add dev eth0 root netem delay 40ms loss 0.5%
   done
   ```

4. **Collect metrics**

   ```bash
   python scripts/collect-metrics.py \
     --endpoint http://localhost:9898/metrics \
     --interval 5 \
     --duration 600 \
     --out experiments/local.csv
   ```

5. **Generate plots (optional)**

   ```bash
   python scripts/generate-plots.py --input experiments/local.csv --output-dir experiments/plots
   ```

6. **Inspect & publish artefacts**

   Generated CSV and PNG files are placed in `experiments/plots/`. The repository includes a published example under `docs/artifacts/` for quick reference.

7. **Cleanup**

   ```bash
   docker compose -f deploy/docker/docker-compose.yml down
   ```

## k3d Cluster Experiment (up to 70 nodes)

1. **Provision cluster**

   ```bash
   scripts/launch-k3d.sh
   ```

   This builds/pushes the `rustchatroom/bootstrap` image, creates the k3d cluster, deploys the bootstrap node, and installs kube-prometheus-stack.

2. **Patch Grafana**

   Port-forward Grafana and import the dashboard config:

   ```bash
   kubectl port-forward svc/prometheus-grafana -n rustchatroom 3000:80
   ```

   Open http://localhost:3000, add the Prometheus datasource, and import `deploy/observability/grafana-dashboard.json`.

3. **Scale workload**

   ```bash
   kubectl scale deployment/chat-nodes -n rustchatroom --replicas=67
   ```

4. **Apply network shaping**

   ```bash
   for pod in $(kubectl get pods -n rustchatroom -l app=chat-node -o name); do
     kubectl exec -n rustchatroom "$pod" -- tc qdisc add dev eth0 root netem delay 45ms 10ms distribution normal loss 1%
   done
   ```

5. **Capture metrics**

   - Prometheus scrape targets are auto-discovered.
   - Use the Grafana dashboard to monitor peer count, gossip throughput, and RTT.
   - Export raw metrics via `kubectl port-forward svc/prometheus-kube-prometheus-prometheus -n rustchatroom 9090` and `scripts/collect-metrics.py`.

6. **Tear down**

   ```bash
   k3d cluster delete rustchatroom
   k3d registry delete rustchatroom-registry
   ```

## Reading the Metrics

- `p2p_node_peers`: connected peers gauge; expects ~alpha\*replicas.
- `p2p_node_gossip_published/received`: counters incremented per GossipSub message.
- `p2p_node_ping_latency_ms`: last RTT sample from libp2p ping.
- `p2p_node_hole_punch_attempts/success`: UDP hole-punch lifecycle metrics.

## Suggested Analysis

Use Jupyter or your favourite notebook tool to import the generated CSV and compute:

- Message delivery latency (publish timestamp vs receive timestamp embedded in payload).
- Throughput per node (derivative of the counters).
- Effect of netem parameters on connectivity (peer gauge).

Example snippet:

```python
import pandas as pd

df = pd.read_csv("experiments/local.csv", parse_dates=["timestamp"])
df.set_index("timestamp", inplace=True)
df[["gossip_published", "gossip_received"]].diff().plot()
```

## Troubleshooting

- If nodes fail to dial bootstrap, ensure the multiaddr is correct and the bootstrap container is reachable.
- STUN resolution failures are logged at WARN level; provide public STUN servers via `--stun-server` if running outside Docker/k3d.
- Prometheus ServiceMonitor requires the kube-prometheus-stack release label `release=prometheus`. Adjust to match your deployment.
