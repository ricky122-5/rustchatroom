#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR=$(cd "$(dirname "$0")/.." && pwd)
REGISTRY_NAME="rustchatroom-registry"
REGISTRY_PORT="5200"

if ! command -v k3d >/dev/null 2>&1; then
  echo "k3d is required" >&2
  exit 1
fi

if ! command -v helm >/dev/null 2>&1; then
  echo "helm is required" >&2
  exit 1
fi

echo "[+] Creating local registry ${REGISTRY_NAME}:${REGISTRY_PORT}"
if ! k3d registry list | grep -q "${REGISTRY_NAME}"; then
  k3d registry create "${REGISTRY_NAME}" --port "${REGISTRY_PORT}"
fi

echo "[+] Creating k3d cluster"
k3d cluster create --config "${ROOT_DIR}/deploy/k3d/cluster.yaml"

echo "[+] Building container image"
docker build -t localhost:${REGISTRY_PORT}/rustchatroom/bootstrap:latest -f "${ROOT_DIR}/deploy/docker/Dockerfile" "${ROOT_DIR}"
docker push localhost:${REGISTRY_PORT}/rustchatroom/bootstrap:latest

echo "[+] Loading manifests"
kubectl apply -f "${ROOT_DIR}/deploy/k3d/manifests.yaml"

echo "[+] Installing kube-prometheus-stack"
helm repo add prometheus-community https://prometheus-community.github.io/helm-charts >/dev/null
helm repo update >/dev/null
helm upgrade --install prometheus prometheus-community/kube-prometheus-stack \
  --namespace rustchatroom --create-namespace \
  --set grafana.enabled=true --set prometheus.prometheusSpec.serviceMonitorSelectorNilUsesHelmValues=false

echo "[+] Applying Prometheus scrape config"
kubectl create configmap rustchatroom-prometheus --from-file=prometheus.yml="${ROOT_DIR}/deploy/observability/prometheus.yml" \
  --namespace=rustchatroom --dry-run=client -o yaml | kubectl apply -f -

echo "[+] Import Grafana dashboard"
kubectl create configmap rustchatroom-grafana-dashboard \
  --from-file=rustchatroom-overview.json="${ROOT_DIR}/deploy/observability/grafana-dashboard.json" \
  -n rustchatroom --dry-run=client -o yaml | kubectl apply -f -

echo "Cluster ready. Use \`kubectl get pods -n rustchatroom\` to inspect nodes."

