#!/usr/bin/env python3
"""Create timestamped SQLite archives and upload with rclone (Python 3.11+, Unix)."""
import argparse
from contextlib import closing
import datetime
import fcntl
import hashlib
import json
import os
from pathlib import Path
import re
import sqlite3
import subprocess
import sys
import tarfile
import tempfile
import time
import tomllib
from zoneinfo import ZoneInfo


class BackupError(Exception):
    """An error safe to print without provider responses or secrets."""


def sha256(stream):
    digest = hashlib.sha256()
    for chunk in iter(lambda: stream.read(1024 * 1024), b""):
        digest.update(chunk)
    return digest.hexdigest()


def snapshot(source, destination, timeout):
    deadline = time.monotonic() + timeout

    def progress(_status, _remaining, _total):
        if time.monotonic() >= deadline:
            raise BackupError("SQLite snapshot timed out; retry when writers are less busy")

    with closing(sqlite3.connect(source.as_uri() + "?mode=ro", uri=True)) as reader:
        with closing(sqlite3.connect(destination)) as writer:
            reader.backup(writer, pages=256, progress=progress, sleep=0.1)
            if writer.execute("PRAGMA integrity_check").fetchall() != [("ok",)]:
                raise BackupError("SQLite snapshot failed integrity_check")
            return writer.execute("PRAGMA user_version").fetchone()[0]


def create_archive(source, directory, archive, timestamp, timeout):
    with tempfile.TemporaryDirectory(prefix=".snapshot-", dir=directory) as temporary:
        staging = Path(temporary)
        database = staging / "tasks.sqlite3"
        version = snapshot(source, database, timeout)
        with database.open("rb") as stream:
            digest = sha256(stream)
        manifest = {
            "format_version": 1,
            "backup_date": timestamp[:10],
            "backup_timestamp": timestamp,
            "created_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
            "source": str(source),
            "sqlite_user_version": version,
            "sha256": digest,
        }
        (staging / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
        compressed = staging / "archive.tar.gz"
        with tarfile.open(compressed, "w:gz") as package:
            for name in ("tasks.sqlite3", "manifest.json"):
                package.add(staging / name, arcname=name)
        with compressed.open("rb") as stream:
            os.fsync(stream.fileno())
        # Publish only a complete archive, without replacing an existing file.
        os.link(compressed, archive)
        descriptor = os.open(directory, os.O_RDONLY)
        try:
            os.fsync(descriptor)
        finally:
            os.close(descriptor)


def verify_archive(archive, source):
    with tarfile.open(archive, "r:gz") as package:
        members = package.getmembers()
        if (len(members) != 2 or {m.name for m in members} != {"tasks.sqlite3", "manifest.json"}
                or not all(m.isfile() for m in members)):
            raise BackupError("unexpected backup archive contents")
        info = package.getmember("manifest.json")
        if info.size > 16384:
            raise BackupError("invalid backup manifest")
        manifest = json.load(package.extractfile(info))
        if (not isinstance(manifest, dict) or manifest.get("format_version") != 1
                or manifest.get("source") != str(source)
                or archive.name != f"taskix-{manifest.get('backup_timestamp', manifest.get('backup_date'))}.tar.gz"):
            raise BackupError("backup manifest does not match this source or archive date")
        if sha256(package.extractfile("tasks.sqlite3")) != manifest.get("sha256"):
            raise BackupError("backup archive checksum mismatch")


def arguments():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--config", type=Path,
                        default=Path(os.environ.get("TASKIX_CONFIG", "~/.config/taskix/config.toml")),
                        help="existing Taskix config (default: TASKIX_CONFIG or ~/.config/taskix/config.toml)")
    parser.add_argument("--output-dir", type=Path,
                        help="local archive directory (default: backups/ beside storage.path)")
    parser.add_argument("--remote", required=True, help="named rclone remote and directory, e.g. r2:bucket/taskix/host")
    parser.add_argument("--rclone", default="rclone", help="rclone executable or absolute path")
    parser.add_argument("--rclone-config", type=Path, help="optional rclone config file")
    parser.add_argument("--timezone", help="IANA timezone for archive dates (default: system local timezone)")
    parser.add_argument("--timeout", type=int, default=300, help="snapshot deadline and each upload deadline, seconds")
    return parser.parse_args()


def upload_error(stderr):
    # Extract only allowlisted categories and numeric status, never raw responses.
    status = re.search(r"(?:StatusCode:\s*|HTTP(?:/\d(?:\.\d)?)?\s+)([45]\d{2})\b", stderr)
    http = f" (HTTP {status.group(1)})" if status else ""
    if re.search(r"\bAccessDenied\b|\bAccess Denied\b|\bForbidden\b", stderr, re.IGNORECASE):
        return f"AccessDenied{http}; check bucket name, endpoint, and credential permissions"
    return f"provider error{http}; run rclone copyto manually with --immutable to inspect details"


def run(args):
    if args.timeout <= 0:
        raise BackupError("timeout must be positive")
    if not re.fullmatch(r"[A-Za-z0-9_][A-Za-z0-9_.-]*:[^\r\n]*", args.remote):
        raise BackupError("remote must use a named rclone configuration, without inline credentials")
    remote = args.remote.rstrip("/")
    remote_path = remote.split(":", 1)[1]
    if remote_path.startswith("//") or any(p in (".", "..") for p in remote_path.split("/")):
        raise BackupError("invalid remote directory")
    with args.config.expanduser().open("rb") as stream:
        config = tomllib.load(stream)
    if config.get("schema_version") != 1:
        raise BackupError("unsupported Taskix config schema_version")
    source = Path(config["storage"]["path"]).expanduser()
    if not source.is_absolute() or not source.is_file():
        raise BackupError("source database must be an existing absolute file path")
    directory = (args.output_dir if args.output_dir is not None else source.parent / "backups").expanduser().resolve()
    source = source.resolve()
    directory.mkdir(parents=True, exist_ok=True, mode=0o700)
    with (directory / ".backup.lock").open("a") as lock:
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            raise BackupError("backup is already running for this output directory") from None
        identity = {"source": str(source), "remote": remote}
        identity_path = directory / ".identity.json"
        if identity_path.exists():
            if json.loads(identity_path.read_text()) != identity:
                raise BackupError("output directory belongs to a different database or remote")
        else:
            with identity_path.open("x") as stream:
                json.dump(identity, stream)
        now = datetime.datetime.now(ZoneInfo(args.timezone)) if args.timezone else datetime.datetime.now().astimezone()
        timestamp = now.strftime("%Y-%m-%d-%H%M%S-%f")
        archive = directory / f"taskix-{timestamp}.tar.gz"
        create_archive(source, directory, archive, timestamp, args.timeout)
        # Validate all retained packages before retrying old and new uploads.
        archives = sorted(directory.glob("taskix-*.tar.gz"))
        for retained in archives:
            if retained.is_symlink():
                raise BackupError("backup archive must not be a symbolic link")
            verify_archive(retained, source)
        for retained in archives:
            destination = f"{remote}/{retained.name}" if remote_path else f"{remote}{retained.name}"
            # Destinations must already exist; scoped S3 tokens need not create buckets.
            command = [args.rclone, "copyto", str(retained), destination, "--immutable",
                       "--s3-no-check-bucket", "--retries", "3"]
            if args.rclone_config:
                command.extend(["--config", str(args.rclone_config.expanduser())])
            try:
                with tempfile.TemporaryFile() as diagnostics:
                    result = subprocess.run(command, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                                            stderr=diagnostics, timeout=args.timeout, check=False)
                    if result.returncode:
                        diagnostics.seek(0, os.SEEK_END)
                        diagnostics.seek(max(0, diagnostics.tell() - 65536))
                        detail = upload_error(diagnostics.read().decode("utf-8", errors="replace"))
                        raise BackupError(f"rclone upload failed (exit {result.returncode}): {detail}; "
                                          "local archives retained for retry")
            except subprocess.TimeoutExpired:
                raise BackupError("rclone upload timed out; local archives retained for retry") from None
        print(f"Backup complete: {archive}")


def main():
    os.umask(0o077)
    try:
        run(arguments())
        return 0
    except BackupError as error:
        print(f"taskix-backup: {error}", file=sys.stderr)
    except FileNotFoundError:
        print("taskix-backup: config, source, or rclone executable not found", file=sys.stderr)
    except (OSError, EOFError, ValueError, KeyError, TypeError, sqlite3.Error, tarfile.TarError):
        print("taskix-backup: invalid configuration/archive or filesystem/SQLite failure; local archives retained", file=sys.stderr)
    return 1


if __name__ == "__main__":
    sys.exit(main())
