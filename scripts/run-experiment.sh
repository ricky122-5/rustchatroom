#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR=$(cd "$(dirname "$0")/.." && pwd)
DEFAULT_NODE_COUNT=16
DEFAULT_DELAY=50ms
DEFAULT_LOSS=0.0

usage() {
  cat <<'EOF'
Usage: run-experiment.sh [OPTIONS]

Options:
  -n, --nodes <count>      Number of chat nodes to deploy (default: 16)
  -d, --delay <duration>   One-way network delay to inject via netem (default: 50ms)
  -l, --loss <percent>     Packet loss percentage (default: 0.0)
  -t, --duration <secs>    Experiment duration in seconds (default: 600)
  -o, --output <dir>       Directory to store metrics dumps (default: ./experiments/<timestamp>)
  -h, --help               Show this message
EOF
}

NODE_COUNT=${DEFAULT_NODE_COUNT}
DELAY=${DEFAULT_DELAY}
LOSS=${DEFAULT_LOSS}
DURATION=600
OUTPUT_DIR=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    -n|--nodes)
      NODE_COUNT=$2; shift 2 ;;
    -d|--delay)
      DELAY=$2; shift 2 ;;
    -l|--loss)
      LOSS=$2; shift 2 ;;
    -t|--duration)
      DURATION=$2; shift 2 ;;
    -o|--output)
      OUTPUT_DIR=$2; shift 2 ;;
    -h|--help)
      usage; exit 0 ;;
    *)
      echo "Unknown argument: $1" >&2
      usage
      exit 1 ;;
  esac
done

TIMESTAMP=$(date +%Y%m%d-%H%M%S)
OUTPUT_DIR=${OUTPUT_DIR:-"${ROOT_DIR}/experiments/${TIMESTAMP}"}
mkdir -p "${OUTPUT_DIR}"

echo "[+] Starting Docker compose cluster"
BOOTSTRAP_MULTIADDR="/dns4/bootstrap/tcp/4001/p2p/12D3KooWbootstrap"
export BOOTSTRAP_MULTIADDR

docker compose -f "${ROOT_DIR}/deploy/docker/docker-compose.yml" up -d --build

echo "[+] Waiting for bootstrap metrics endpoint"
until curl -sf http://localhost:9898/metrics >/dev/null; do
  sleep 2
done

echo "[+] Scaling chat service to ${NODE_COUNT}"
docker compose -f "${ROOT_DIR}/deploy/docker/docker-compose.yml" up -d --scale chat=${NODE_COUNT}

echo "[+] Configuring netem on chat containers"
for container in $(docker compose -f "${ROOT_DIR}/deploy/docker/docker-compose.yml" ps -q chat); do
  docker exec "$container" tc qdisc add dev eth0 root netem delay "$DELAY" loss "$LOSS"
done

echo "[+] Collecting metrics for ${DURATION}s"
python3 "${ROOT_DIR}/scripts/collect-metrics.py" \
  --endpoint http://localhost:9898/metrics \
  --duration "${DURATION}" \
  --interval 5 \
  --out "${OUTPUT_DIR}/metrics.csv"

echo "[+] Rendering plots"
python3 "${ROOT_DIR}/scripts/generate-plots.py" --input "${OUTPUT_DIR}/metrics.csv" --output-dir "${OUTPUT_DIR}/plots"

echo "[+] Cleaning up"
docker compose -f "${ROOT_DIR}/deploy/docker/docker-compose.yml" down

echo "Experiment complete. Results stored in ${OUTPUT_DIR}"

