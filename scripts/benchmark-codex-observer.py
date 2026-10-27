#!/usr/bin/env python3
"""Compare legacy and streaming observers in fresh processes; emit raw JSONL."""
import argparse
import json
import os
import platform
import resource
import subprocess
import sys


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", help="release agentix-codex library test executable")
    parser.add_argument("--rounds", type=int, default=5)
    parser.add_argument("--worker", action="store_true", help=argparse.SUPPRESS)
    args = parser.parse_args()
    if args.worker:
        result = subprocess.run(
            [args.binary, "observer_benchmark", "--ignored", "--nocapture"],
            text=True, capture_output=True, check=True,
        )
        rows = [line.removeprefix("OBSERVER_BENCH ") for line in result.stdout.splitlines()
                if line.startswith("OBSERVER_BENCH ")]
        if len(rows) != 1:
            raise RuntimeError(f"Expected one benchmark row: {result.stdout}")
        row = json.loads(rows[0])
        rss = resource.getrusage(resource.RUSAGE_CHILDREN).ru_maxrss
        row["peak_rss_bytes"] = rss if sys.platform == "darwin" else rss * 1024
        row["platform"] = platform.platform()
        print(json.dumps(row))
        return
    cases = [(128, 1), (128, 16), (1024, 1), (16384, 1),
             (1048576, 1), (17 * 1048576, 1), (65 * 1048576, 1)]
    for repeat in range(args.rounds):
        for size, clients in cases:
            order = ("legacy", "stream") if repeat % 2 == 0 else ("stream", "legacy")
            for implementation in order:
                env = dict(os.environ, OBSERVER_IMPL=implementation,
                           OBSERVER_BYTES=str(size), OBSERVER_CLIENTS=str(clients))
                result = subprocess.run(
                    [sys.executable, __file__, args.binary, "--worker"], env=env,
                    text=True, capture_output=True, check=True,
                )
                row = json.loads(result.stdout)
                row["round"] = repeat
                print(json.dumps(row), flush=True)


if __name__ == "__main__":
    main()
