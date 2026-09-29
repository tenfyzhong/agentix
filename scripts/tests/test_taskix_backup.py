"""Regression tests for the standalone backup script (no cloud credentials)."""
import datetime
import fcntl
import hashlib
import io
import json
import os
from pathlib import Path
import sqlite3
import subprocess
import sys
import tarfile
import tempfile
import unittest

SCRIPT = Path(__file__).resolve().parents[1] / "taskix-backup.py"


class BackupTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.database = self.root / "source.sqlite3"
        self.connection = sqlite3.connect(self.database)
        self.addCleanup(self.connection.close)
        self.connection.execute("PRAGMA journal_mode=WAL")
        self.connection.execute("CREATE TABLE evidence (value TEXT)")
        self.connection.execute("INSERT INTO evidence VALUES ('committed')")
        self.connection.commit()
        self.config = self.root / "taskix.toml"
        self.config.write_text(f'schema_version = 1\n[storage]\npath = {json.dumps(str(self.database))}\n')
        self.output = self.root / "backups"
        self.rclone = self.root / "rclone"
        self.rclone.write_text(f'''#!{sys.executable}
import os, pathlib, sys
root = pathlib.Path(os.environ["MOCK_ROOT"])
with (root / "calls").open("a") as output:
    output.write(__import__("json").dumps(sys.argv[1:]) + "\\n")
if os.environ.get("MOCK_FAIL"):
    print(os.environ.get("MOCK_ERROR", "provider-secret"), file=sys.stderr)
    sys.exit(7)
(root / "uploaded.tar.gz").write_bytes(pathlib.Path(sys.argv[2]).read_bytes())
''')
        self.rclone.chmod(0o700)

    def run_backup(self, *args, fail=False, default_output=False, error="provider-secret"):
        output_args = [] if default_output else ["--output-dir", str(self.output)]
        return subprocess.run(
            [sys.executable, str(SCRIPT), "--config", str(self.config),
             *output_args, "--remote", "r2:bucket/taskix/host",
             "--rclone", str(self.rclone), *args],
            env={**os.environ, "MOCK_ROOT": str(self.root), "MOCK_FAIL": "1" if fail else "", "MOCK_ERROR": error},
            capture_output=True, text=True, timeout=15,
        )

    def assert_success(self, result):
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_omitted_output_creates_backups_next_to_database(self):
        self.assertFalse(self.output.exists())
        self.assert_success(self.run_backup(default_output=True))
        archives = list((self.database.parent / "backups").glob("*.tar.gz"))
        self.assertEqual(len(archives), 1)
        calls = [json.loads(line) for line in (self.root / "calls").read_text().splitlines()]
        self.assertEqual(Path(calls[0][1]).resolve(), archives[0].resolve())

    def test_explicit_output_overrides_database_default(self):
        custom = self.root / "custom" / "archives"
        self.assert_success(self.run_backup("--output-dir", str(custom)))
        self.assertEqual(len(list(custom.glob("*.tar.gz"))), 1)
        self.assertFalse((self.database.parent / "backups").exists())

    def test_wal_snapshot_can_be_restored_and_each_run_creates_a_new_archive(self):
        self.connection.execute("INSERT INTO evidence VALUES ('uncommitted')")
        self.assert_success(self.run_backup())
        archives = list(self.output.glob("*.tar.gz"))
        self.assertEqual(len(archives), 1)
        archive = archives[0]
        self.assertRegex(archive.name, r"^taskix-\d{4}-\d{2}-\d{2}-\d{6}-\d{6}\.tar\.gz$")
        original = archive.read_bytes()
        with tarfile.open(archive) as package:
            self.assertEqual(set(package.getnames()), {"tasks.sqlite3", "manifest.json"})
            database = package.extractfile("tasks.sqlite3").read()
            manifest = json.load(package.extractfile("manifest.json"))
        self.assertEqual(manifest["sha256"], hashlib.sha256(database).hexdigest())
        restored = self.root / "restored.sqlite3"
        restored.write_bytes(database)
        with sqlite3.connect(restored) as connection:
            self.assertEqual(connection.execute("SELECT value FROM evidence").fetchall(), [("committed",)])
            self.assertEqual(connection.execute("PRAGMA integrity_check").fetchone(), ("ok",))
        self.connection.rollback()
        self.connection.execute("INSERT INTO evidence VALUES ('later')")
        self.connection.commit()
        self.assert_success(self.run_backup())
        self.assertEqual(original, archive.read_bytes())
        archives = sorted(self.output.glob("*.tar.gz"))
        self.assertEqual(len(archives), 2)
        with tarfile.open(archives[-1]) as package:
            restored.write_bytes(package.extractfile("tasks.sqlite3").read())
        with sqlite3.connect(restored) as connection:
            self.assertEqual(connection.execute("SELECT value FROM evidence").fetchall(),
                             [("committed",), ("later",)])
        calls = [json.loads(line) for line in (self.root / "calls").read_text().splitlines()]
        self.assertEqual(len(calls), 3)
        self.assertEqual(calls[0][0], "copyto")
        self.assertIn("--immutable", calls[0])
        self.assertIn("--s3-no-check-bucket", calls[0])
        self.assertEqual(calls[0][2], f"r2:bucket/taskix/host/{archive.name}")
        self.assertEqual(archive.stat().st_mode & 0o777, 0o600)

    def test_upload_failure_preserves_archive_for_retry_without_leaking_stderr(self):
        result = self.run_backup(fail=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("rclone upload failed", result.stderr)
        self.assertNotIn("provider-secret", result.stdout + result.stderr)
        archive = next(self.output.glob("*.tar.gz"))
        original = archive.read_bytes()
        self.assert_success(self.run_backup())
        self.assertEqual(original, archive.read_bytes())

    def test_access_denied_reports_actionable_reason_without_provider_secrets(self):
        result = self.run_backup(fail=True, error=(
            "operation error S3: ListObjectsV2, https response error StatusCode: 403, "
            "api error AccessDenied: Access Denied; token=provider-secret"))
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("AccessDenied", result.stderr)
        self.assertIn("HTTP 403", result.stderr)
        self.assertIn("bucket", result.stderr)
        self.assertNotIn("provider-secret", result.stdout + result.stderr)

    def test_unknown_provider_error_has_manual_diagnostic_guidance(self):
        result = self.run_backup(fail=True)
        self.assertIn("rclone copyto", result.stderr)
        self.assertNotIn("provider-secret", result.stdout + result.stderr)

    def test_missing_database_is_not_created(self):
        self.connection.close()
        self.database.unlink()
        result = self.run_backup()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("source database", result.stderr)
        self.assertFalse(self.database.exists())
        self.assertFalse((self.root / "calls").exists())

    def test_process_lock_prevents_overlapping_backup(self):
        self.output.mkdir()
        with (self.output / ".backup.lock").open("w") as lock:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            result = self.run_backup()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("already running", result.stderr)
        self.assertFalse((self.root / "calls").exists())

    def test_corrupt_existing_archive_is_not_uploaded_or_overwritten(self):
        self.output.mkdir()
        archive = self.output / f"taskix-{datetime.date.today().isoformat()}.tar.gz"
        archive.write_bytes(b"broken")
        self.assertNotEqual(self.run_backup().returncode, 0)
        self.assertEqual(archive.read_bytes(), b"broken")
        self.assertFalse((self.root / "calls").exists())

    def test_database_and_remote_identity_cannot_share_an_output_directory(self):
        self.assert_success(self.run_backup())
        result = self.run_backup("--remote", "s3:other-bucket/backups")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("different database or remote", result.stderr)

    def test_inline_credentials_and_local_destination_are_rejected(self):
        for remote in [":s3,key=secret:bucket", "/tmp/backups", "https://user:secret@host/path"]:
            with self.subTest(remote=remote):
                result = self.run_backup("--remote", remote)
                self.assertNotEqual(result.returncode, 0)
                self.assertNotIn("secret", result.stdout + result.stderr)
                self.assertFalse((self.root / "calls").exists())

    def test_older_failed_archives_are_retried_with_today(self):
        self.assert_success(self.run_backup())
        today = next(self.output.glob("*.tar.gz"))
        yesterday = (datetime.date.today() - datetime.timedelta(days=1)).isoformat()
        with tarfile.open(today) as package:
            database = package.extractfile("tasks.sqlite3").read()
            manifest = json.load(package.extractfile("manifest.json"))
        manifest["backup_date"] = yesterday
        manifest.pop("backup_timestamp", None)
        previous = self.output / f"taskix-{yesterday}.tar.gz"
        with tarfile.open(previous, "w:gz") as package:
            for name, content in [("tasks.sqlite3", database),
                                  ("manifest.json", json.dumps(manifest).encode())]:
                entry = tarfile.TarInfo(name)
                entry.size = len(content)
                package.addfile(entry, io.BytesIO(content))
        (self.root / "calls").unlink()
        self.assert_success(self.run_backup())
        calls = [json.loads(line) for line in (self.root / "calls").read_text().splitlines()]
        self.assertEqual(len(calls), 3)
        self.assertEqual([Path(call[1]).name for call in calls[:2]], [previous.name, today.name])

    def test_truncated_archive_reports_a_clean_error_without_uploading(self):
        self.assert_success(self.run_backup())
        archive = next(self.output.glob("*.tar.gz"))
        archive.write_bytes(archive.read_bytes()[:20])
        (self.root / "calls").unlink()
        result = self.run_backup()
        self.assertNotEqual(result.returncode, 0)
        self.assertNotIn("Traceback", result.stderr)
        self.assertFalse((self.root / "calls").exists())


if __name__ == "__main__":
    unittest.main()
