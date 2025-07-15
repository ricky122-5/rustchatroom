#!/usr/bin/env python3
import argparse
import csv
import time
from datetime import datetime

import requests


def parse_args():
    parser = argparse.ArgumentParser(description="Collect Prometheus metrics snapshots")
    parser.add_argument("--endpoint", required=True, help="Prometheus /metrics endpoint")
    parser.add_argument("--interval", type=int, default=5, help="Scrape interval seconds")
    parser.add_argument("--duration", type=int, default=300, help="Total duration seconds")
    parser.add_argument("--out", required=True, help="CSV file path to write results")
    return parser.parse_args()


def scrape(endpoint):
    resp = requests.get(endpoint, timeout=5)
    resp.raise_for_status()
    return resp.text


def extract(metrics_text, metric_name):
    for line in metrics_text.splitlines():
        if line.startswith(metric_name):
            parts = line.split()
            if len(parts) == 2:
                return float(parts[1])
    return None


def main():
    args = parse_args()
    fields = [
        "timestamp",
        "gossip_published",
        "gossip_received",
        "peers",
        "ping_latency_ms",
    ]

    with open(args.out, "w", newline="") as handle:
        writer = csv.writer(handle)
        writer.writerow(fields)

        start = time.time()
        while True:
            now = time.time()
            if now - start > args.duration:
                break

            metrics = scrape(args.endpoint)
            row = [
                datetime.utcnow().isoformat(),
                extract(metrics, "p2p_node_gossip_published"),
                extract(metrics, "p2p_node_gossip_received"),
                extract(metrics, "p2p_node_peers"),
                extract(metrics, "p2p_node_ping_latency_ms"),
            ]
            writer.writerow(row)
            handle.flush()
            time.sleep(args.interval)


if __name__ == "__main__":
    main()

