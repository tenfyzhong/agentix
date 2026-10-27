#!/usr/bin/env python3
"""Run identical registry-verified relay matrices, alternating old/new binaries."""
import argparse
import json
import hashlib
from pathlib import Path
import os
import subprocess


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("baseline")
    parser.add_argument("stream")
    parser.add_argument("--rounds", type=int, default=5)
    args = parser.parse_args()
    binaries = {"baseline": args.baseline, "stream": args.stream}
    hashes = {name: hashlib.sha256(Path(binary).read_bytes()).hexdigest()
              for name, binary in binaries.items()}
    if hashes["baseline"] == hashes["stream"]:
        raise ValueError("Baseline and streaming binaries must differ")
    for repeat in range(args.rounds):
        order = ("baseline", "stream") if repeat % 2 == 0 else ("stream", "baseline")
        for implementation in order:
            result = subprocess.run(
                [binaries[implementation], "compare_direct_and_proxy", "--ignored", "--nocapture"],
                env=dict(os.environ, PROXY_BENCH_ROUNDS="1"),
                capture_output=True, text=True, check=True,
            )
            rows = [json.loads(line.removeprefix("PROXY_BENCH "))
                    for line in result.stdout.splitlines() if line.startswith("PROXY_BENCH ")]
            if len(rows) != 48 or not all(row["tracking_verified"] for row in rows):
                raise RuntimeError(f"Incomplete or invalid relay matrix: {result.stdout}")
            for row in rows:
                row.update(round=repeat, implementation=implementation, binary_sha256=hashes[implementation])
                print(json.dumps(row), flush=True)


if __name__ == "__main__":
    main()
