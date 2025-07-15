#!/usr/bin/env python3
import argparse
from pathlib import Path

import matplotlib.pyplot as plt
import pandas as pd


def parse_args():
    parser = argparse.ArgumentParser("Generate PNG plots from metrics CSV")
    parser.add_argument("--input", required=True, help="CSV produced by collect-metrics.py")
    parser.add_argument("--output-dir", required=True, help="Directory to write PNG files")
    return parser.parse_args()


def main():
    args = parse_args()
    csv_path = Path(args.input)
    output_dir = Path(args.output_dir)
    output_dir.mkdir(parents=True, exist_ok=True)

    df = pd.read_csv(csv_path, parse_dates=["timestamp"])
    df.set_index("timestamp", inplace=True)

    if {"gossip_published", "gossip_received"}.issubset(df.columns):
        ax = df[["gossip_published", "gossip_received"]].diff().fillna(0).plot()
        ax.set_title("Gossip Throughput (msgs/s)")
        ax.set_ylabel("msgs/s")
        plt.tight_layout()
        plt.savefig(output_dir / "gossip_throughput.png")
        plt.close()

    if "peers" in df.columns:
        ax = df["peers"].plot()
        ax.set_title("Connected Peers")
        ax.set_ylabel("peers")
        plt.tight_layout()
        plt.savefig(output_dir / "peers.png")
        plt.close()

    if "ping_latency_ms" in df.columns:
        ax = df["ping_latency_ms"].plot()
        ax.set_title("Ping Latency")
        ax.set_ylabel("ms")
        plt.tight_layout()
        plt.savefig(output_dir / "ping_latency.png")
        plt.close()

    if {"hole_punch_attempts", "hole_punch_success"}.issubset(df.columns):
        ax = df[["hole_punch_attempts", "hole_punch_success"]].diff().fillna(0).plot()
        ax.set_title("Hole Punch Attempts")
        ax.set_ylabel("count/s")
        plt.tight_layout()
        plt.savefig(output_dir / "hole_punch.png")
        plt.close()


if __name__ == "__main__":
    main()

