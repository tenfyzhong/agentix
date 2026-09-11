"""Drive an isolated native Codex TUI through a PTY for the Rust mock benchmark."""
import errno
import fcntl
import json
import os
from pathlib import Path
import pty
import select
import signal
import struct
import subprocess
import sys
import termios
import time


def run(endpoint, directory):
    home = Path(directory) / "codex-home"
    home.mkdir(exist_ok=True)
    (home / "config.toml").write_text('model = "gpt-6"\n')
    master, slave = pty.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 40, 160, 0, 0))
    env = os.environ.copy()
    env.update(CODEX_HOME=str(home), TERM="xterm-256color")
    env.pop("CODEX_THREAD_ID", None)
    env.pop("CODEX_INTERNAL_SESSION_ID", None)
    command = [os.environ.get("CODEX_BENCH_BINARY", "codex"), "--remote", endpoint,
               "--no-alt-screen", "-C", directory, "proxy benchmark"]
    start = time.monotonic_ns()
    child = subprocess.Popen(command, stdin=slave, stdout=slave, stderr=slave,
                             env=env, cwd=directory, start_new_session=True)
    os.close(slave)
    transcript = bytearray()
    submitted = None
    first = None
    ready_ms = None
    try:
        while time.monotonic_ns() - start < 20_000_000_000:
            if not select.select([master], [], [], 0.05)[0]:
                continue
            try:
                data = os.read(master, 65536)
            except OSError as error:
                if error.errno == errno.EIO:
                    break
                raise
            if not data:
                break
            transcript.extend(data)
            if b"\x1b[6n" in data:
                os.write(master, b"\x1b[1;1R")
            if submitted is None and b"BENCHMARK_READY" in transcript:
                ready_ms = (time.monotonic_ns() - start) / 1e6
                time.sleep(0.1)
                transcript.clear()
                os.write(master, b"\x1b[200~measure this turn\x1b[201~")
                time.sleep(0.1)
                submitted = time.monotonic_ns()
                os.write(master, b"\r")
            if submitted is not None and first is None and b"BENCHMARK_FIRST" in transcript:
                first = (time.monotonic_ns() - submitted) / 1e6
            if submitted is not None and b"BENCHMARK_COMPLETE" in transcript:
                if first is None:
                    raise RuntimeError("completion arrived without first output")
                return {"launch_to_ready_ms": ready_ms, "input_to_first_ms": first,
                        "input_to_complete_ms": (time.monotonic_ns() - submitted) / 1e6}
        raise RuntimeError(transcript.decode(errors="replace"))
    finally:
        if child.poll() is None:
            os.killpg(child.pid, signal.SIGTERM)
        try:
            child.wait(timeout=3)
        except subprocess.TimeoutExpired:
            os.killpg(child.pid, signal.SIGKILL)
            child.wait()
        os.close(master)


if __name__ == "__main__":
    print(json.dumps(run(*sys.argv[1:])))
