# Taskix database backups with rclone

[`scripts/taskix-backup.py`](../scripts/taskix-backup.py) creates SQLite
snapshots on every run, packages it as `taskix-YYYY-MM-DD-HHMMSS-ffffff.tar.gz`, and uploads
it with rclone. It is a standalone script: no `taskix backup` command, backup-specific
configuration, Rust dependencies, or running Taskix service are required. Scheduling is provided by launchd or cron.

## Requirements and scope

- macOS or Linux, Python 3.11+ with SQLite and timezone data, and rclone on PATH
  (or specify its absolute path with `--rclone`). No pip packages are required.
- An existing Taskix configuration with `schema_version = 1` and an absolute
  `[storage].path`; `~` is expanded. The script reads this configuration without
  changing it and does not require access to the Obsidian vault.
- A configured, writable named rclone remote that supports `rclone copyto`.
  Provision any required bucket, container, share or directory first. Give each
  machine/database its own remote directory and local archive directory.

Without a memory database, the archive remains the version-1 pair of
`tasks.sqlite3` and `manifest.json`. When a memory database exists, version 2
also contains `memory.sqlite3`, its schema version and SHA-256, and source
coverage metadata. The memory path comes from `[memory.storage].path`, or
`memory.sqlite3` beside `[storage].path`. Existing memory is included even when
`TASKIX_MEMORY_ENABLED` is unset or disabled. An explicitly configured or enabled
memory database that is missing is an error; initialize it before backing up. Both paths must be
distinct absolute paths. Old single-database archives remain valid for upload
retry and restore in the same output directory.

The memory snapshot is taken **before** the task snapshot. Every source receipt
in memory is compared with the later task snapshot, including source instance,
sequence, ownership, revision and message content. A mismatched pair fails before
publication. This is a recoverable ordered pair, not a cross-database transaction:
later task receipts are replayed by the memory service, including acknowledged
ones. Source outbox records are retained for this recovery protocol. Startup
also validates existing memory receipts in bounded pages and rejects a task
history that is older than or incompatible with its memory history.

Configuration, credentials, Obsidian documents, attachments, and the separate Jev
metrics database are **not included**. For a matched database-and-vault recovery
point, follow the [full backup procedure](task-board.md#data-coverage-and-recovery):
pause writers and note edits while capturing both. Restoring an old memory
snapshot also restores its historical human edits and forgotten-memory markers;
edits or forget operations made after that snapshot cannot be recovered from
task conversation receipts alone.

The script uses Python's [SQLite online backup API](https://docs.python.org/3/library/sqlite3.html#sqlite3.Connection.backup)
and checks `PRAGMA integrity_check`. Committed WAL data is included without
copying a live database's main file directly. Concurrent writers may extend
snapshot time; `--timeout` bounds snapshot copying and each rclone invocation
(default 300 seconds). Compression and archive verification are not subject to
that deadline. Allow local disk space for the snapshot and compressed archive.

## Configure a remote

The script delegates uploads to rclone; it is not limited to S3, R2 and WebDAV.
Writable named remotes supported by `rclone copyto` can be used without changing
the script. Consult the [rclone backend overview](https://rclone.org/overview/)
for the full list and backend capabilities.

Run `rclone config` interactively, then use one of these named remotes. Names
such as `gdrive` or `sftp` below are examples you choose during configuration,
not built-in remote names:

| Destination | rclone configuration | Script argument example |
| --- | --- | --- |
| Amazon S3 | Storage `s3`, provider `AWS`, bucket region and credentials | `--remote s3:my-bucket/taskix/macbook` |
| Cloudflare R2 | Storage `s3`, provider `Cloudflare`, R2 access key pair, account S3 endpoint | `--remote r2:my-bucket/taskix/macbook` |
| WebDAV | Storage `webdav`, HTTPS server URL, vendor and authentication | `--remote dav:backups/taskix/macbook` |
| Google Drive | Storage `drive`, OAuth authorization | `--remote gdrive:taskix-backups` |
| Microsoft OneDrive | Storage `onedrive`, OAuth authorization | `--remote onedrive:taskix-backups` |
| Dropbox | Storage `dropbox`, OAuth authorization | `--remote dropbox:taskix-backups` |
| SFTP | Storage `sftp`, host, user and SSH authentication | `--remote sftp:backups/taskix` |
| FTP | Storage `ftp`, server and authentication; use TLS where supported | `--remote ftp:backups/taskix` |
| SMB | Storage `smb`, server, share and authentication | `--remote nas:share/taskix-backups` |
| Azure Blob Storage | Storage `azureblob`, account authentication and container | `--remote azure:container/taskix` |
| Google Cloud Storage | Storage `google cloud storage`, project authentication and bucket | `--remote gcs:my-bucket/taskix` |
| Backblaze B2 | Storage `b2`, application key and bucket | `--remote b2:my-bucket/taskix` |
| Encrypted remote | Storage `crypt`, wrapping an already configured remote | `--remote encrypted:taskix-backups` |

Each backend has its own path and authentication rules: cloud drives generally
use directory paths, object stores include a bucket/container, and SMB includes
a share. Read-only remotes cannot be backup destinations. Hash support, timestamp
precision and permission requirements differ, so validate a transfer and restore
on your chosen backend before relying on scheduled backups. The script's S3-only
`--s3-no-check-bucket` flag does not configure or change other backend types.

An encrypted `crypt` remote encrypts data uploaded to its underlying storage.
Keep its configuration and encryption password recoverable separately; they are
not included in these archives. Local archives remain unencrypted.

For R2, use `https://<ACCOUNT_ID>.r2.cloudflarestorage.com` as the endpoint and
`auto` as the region where requested. Use R2 S3 credentials, not a public bucket
URL. See the official [rclone S3/R2](https://rclone.org/s3/#cloudflare-r2) and
[WebDAV](https://rclone.org/webdav/) instructions.

Keep credentials in rclone's configuration or supported environment variables.
Pass `--rclone-config /absolute/path/rclone.conf` when needed. The script accepts
named remotes only, not inline credential connection strings. Scheduled jobs
must have access to the same rclone configuration and any required environment
variables; an encrypted rclone configuration must be usable without a prompt.

### Switch remote targets

The local output directory is bound to its source database and remote. When
switching targets, use a new `--output-dir` so that the existing directory and
pending uploads retain their original destination. For example, after configuring
`gdrive`:

```sh
python3 scripts/taskix-backup.py \
  --remote gdrive:taskix-backups \
  --output-dir "$HOME/.local/share/taskix/backups-gdrive"
```

This creates a fresh snapshot for the new target; it does not migrate old remote
backups. The default database-adjacent `backups/` directory remains convenient
when using one target.

## Run manually

From a repository checkout:

```sh
python3 scripts/taskix-backup.py \
  --config "$HOME/.config/taskix/config.toml" \
  --remote r2:my-bucket/taskix/macbook \
  --timezone Asia/Shanghai
```

Omit `--config` to use `TASKIX_CONFIG`, falling back to
`~/.config/taskix/config.toml`. `--output-dir` is optional: by default the script
creates `backups/` beside the configured database file. For example,
`storage.path = "~/.local/share/taskix/tasks.sqlite3"` produces archives under
`~/.local/share/taskix/backups/`. An explicit `--output-dir` overrides this location;
missing directories are created automatically. Databases sharing a parent
directory need distinct explicit output directories to preserve target isolation.
Use `--help` for all options. No actual scheduler
or remote is installed by running the script.

Behavior:

- Every run publishes a new complete archive atomically. Names include hours,
  minutes, seconds and microseconds, allowing multiple backups in one day or
  second. The default timezone is the system timezone; `--timezone` selects an
  IANA zone. Existing packages are never overwritten.
- Every run validates all retained archives and their checksums before uploading.
  Older date-only archives are still accepted and retried.
- Every run retries all retained archives, including earlier failed runs,
  using `rclone copyto --immutable`. Matching remote objects are skipped by
  rclone; conflicting objects cause failure. Comparison capabilities depend on
  the backend. A successful transfer is not an independent restore drill.
- S3/R2 uploads include `--s3-no-check-bucket`, since the destination bucket must
  already exist. This avoids rclone's bucket-creation request when credentials
  are scoped to object operations. The flag has no effect on WebDAV.
- A lock prevents overlapping runs that use the same local directory. The lock
  is released by the OS when the process exits; do not delete its lock file.
- A local identity file prevents accidental reuse of a directory with another
  source database or remote. Use a new directory when changing either.
- Failures return nonzero and preserve completed archives. No archives are
  deleted automatically, locally or remotely. A corrupt retained archive stops
  the run; inspect and move it aside before retrying. An interrupted compression
  may leave a hidden `.snapshot-*` directory; remove it only when no run is active.

Uploads use [copyto](https://rclone.org/commands/rclone_copyto/), never `sync`, so
removing an old local archive does not delete its remote copy. Prune older local files
only after confirming remote copies; use a deliberate remote lifecycle policy
for retention. Retained archives are rechecked every run, so large collections
increase runtime.

## Schedule daily on macOS

Install a stable copy of the script, independent of a development worktree:

```sh
mkdir -p "$HOME/.local/bin" "$HOME/Library/Logs/taskix" "$HOME/Library/LaunchAgents"
install -m 700 scripts/taskix-backup.py "$HOME/.local/bin/taskix-backup.py"
```

Save the following as `~/Library/LaunchAgents/local.taskix.backup.plist`.
Replace `/Users/YOU`, the remote, and the Python/rclone executable paths with
your actual paths. Use Python 3.11+; launchd does not expand `~` or shell variables
in these values. The example uses Apple Silicon Homebrew paths.

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>local.taskix.backup</string>
  <key>ProgramArguments</key>
  <array>
    <string>/opt/homebrew/bin/python3</string>
    <string>/Users/YOU/.local/bin/taskix-backup.py</string>
    <string>--config</string><string>/Users/YOU/.config/taskix/config.toml</string>
    <string>--remote</string><string>r2:my-bucket/taskix/macbook</string>
    <string>--rclone</string><string>/opt/homebrew/bin/rclone</string>
    <string>--rclone-config</string><string>/Users/YOU/.config/rclone/rclone.conf</string>
    <string>--timezone</string><string>Asia/Shanghai</string>
  </array>
  <key>StartCalendarInterval</key>
  <dict><key>Hour</key><integer>3</integer><key>Minute</key><integer>0</integer></dict>
  <key>StandardOutPath</key><string>/Users/YOU/Library/Logs/taskix/backup.log</string>
  <key>StandardErrorPath</key><string>/Users/YOU/Library/Logs/taskix/backup-error.log</string>
</dict>
</plist>
```

Load it, run once, and inspect the job:

```sh
launchctl bootstrap "gui/$(id -u)" "$HOME/Library/LaunchAgents/local.taskix.backup.plist"
launchctl kickstart "gui/$(id -u)/local.taskix.backup"
launchctl print "gui/$(id -u)/local.taskix.backup"
```

These command examples use POSIX shell substitution; in fish, use `(id -u)`
instead of `$(id -u)`. To disable scheduling:

```sh
launchctl bootout "gui/$(id -u)" "$HOME/Library/LaunchAgents/local.taskix.backup.plist"
```

The schedule is 03:00 in the **system timezone**; `--timezone` controls archive
dates, not launchd scheduling. A LaunchAgent runs for the logged-in user.
[launchd catches up after sleep](https://developer.apple.com/library/archive/documentation/MacOSX/Conceptual/BPSystemStartup/Chapters/ScheduledJobs.html),
but it cannot recover database history for days when the machine was off. There
is no automatic upload-failure retry schedule; rerun manually or add additional
scheduled attempts. Each attempt creates a fresh snapshot and retries retained packages.

## Schedule daily on Linux

Install the script at a stable path and add a line with `crontab -e`:

```cron
0 3 * * * /usr/bin/python3 /home/YOU/.local/bin/taskix-backup.py --config /home/YOU/.config/taskix/config.toml --remote s3:my-bucket/taskix/server --rclone /usr/bin/rclone --rclone-config /home/YOU/.config/rclone/rclone.conf >> /home/YOU/taskix-backup.log 2>&1
```

Use actual executable paths and Python 3.11+. Cron normally uses the machine's
timezone and may miss executions while the machine is asleep or off. Ensure the
job's user can read the database/configuration and write its output directory.

## Inspect and restore

Download an archive into an empty working directory:

```sh
rclone copyto r2:my-bucket/taskix/macbook/taskix-2026-09-29-030000-123456.tar.gz ./taskix-2026-09-29-030000-123456.tar.gz
tar -tzf taskix-2026-09-29-030000-123456.tar.gz
```

Restore into a new directory using the same script (no config, rclone or remote
is needed for this mode):

```sh
python3 scripts/taskix-backup.py \
  --restore ./taskix-2026-09-29-030000-123456.tar.gz \
  --restore-dir ./restored-taskix
```

The script rejects unexpected members, symbolic links, duplicates, invalid
manifests, checksum failures and incompatible source histories. It streams files
into private staging, checks SQLite integrity and coverage, then atomically
publishes the whole directory without replacing an existing path. macOS uses
`renamex_np(RENAME_EXCL)`; Linux requires libc/filesystem `renameat2` support.
The archive must keep its original timestamped filename. SHA-256 detects damage;
it does not authenticate an archive obtained from an untrusted party.

Stop Taskix/Agentix writers, the memory service, host hooks, projection imports
and backup schedules before switching databases. Preserve existing databases and
their `-wal`/`-shm` files together in a separate rollback directory. Point
`[storage].path` and `[memory.storage].path` at the verified restored files, or
install both snapshots while every writer is stopped; never combine an old WAL
with a restored main file. The script does not change configuration or live data.
Restart memory service and inspect `taskix memory status`, `work` and `source`.
It validates the pair before starting workers and resumes durable queue entries
after lease expiry. A restored older memory snapshot can replay newer retained
task sources; incompatible/forked task histories are rejected, not silently reset.
Replay resumes from the memory database's durable ordered checkpoint. A failed
input keeps that checkpoint in place even when later sources have already arrived.
Older snapshots without a checkpoint replay idempotently from zero to repair
possible gaps. The archive's `coverage.replay_after` is the maximum sequence
observed during pair validation, not the service's recovery checkpoint.

Use a compatible Taskix version and inspect state before resuming normal work.
Historical Task leases must not be reused. `taskix doctor` and `taskix sync`
inspect and rebuild board projections. Memory notes are read-only projections:
`taskix memory sync` rebuilds them from the restored memory database. Preserve
any local edits separately before synchronization; they are not imported as
memory revisions.

For transfer failures, the script reports the rclone exit status, a recognized
access-denied category and HTTP status when available, and retains local archives.
Only these diagnostic fields are extracted; raw provider responses are not printed
to scheduled logs. Unknown errors include a manual-diagnosis hint.

For example, `AccessDenied (HTTP 403)` means the provider rejected a request; it
does not by itself establish which setting is wrong. For S3/R2,
`--remote taskix-backup:db` means remote **taskix-backup**, bucket **db**.
If `db` is intended as a prefix in another bucket, use
`taskix-backup:ACTUAL-BUCKET/db` and a new local `--output-dir` (the old directory
is bound to its original remote). Verify the endpoint, credentials and access to
that bucket. An R2 token should have Object Read & Write access scoped to the
intended bucket; see [R2 authentication](https://developers.cloudflare.com/r2/api/tokens/).

Use a read-only check such as `rclone lsf taskix-backup:db --max-depth 1` with
the same rclone configuration. A failed listing proves only that the listing
request failed, not that every object operation is forbidden. Diagnose the
actual upload using a manual `rclone copyto` of the retained archive
to the same destination with `--immutable --s3-no-check-bucket` and your normal
configuration. A `CreateBucket` 403 with a known existing bucket is a reason to
use this flag, not to grant bucket-creation access. Do not
publish unredacted diagnostic logs.

## Development verification

```sh
make test-backup
```

Tests use real SQLite WAL databases, compression and restore checks, and a fake
rclone executable. They cover multiple snapshots per day, retries, locking, invalid sources,
corrupt archives and target isolation. They do not establish live provider
authentication, bucket policy, WebDAV compatibility, or scheduler installation.

A live upload and download comparison has been verified against one configured
S3-compatible destination. Other backend examples describe rclone capabilities;
they have not each been tested with this script. This is not a claim of live
acceptance across all rclone providers.
