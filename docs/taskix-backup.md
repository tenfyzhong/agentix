# Taskix database backups with rclone

The user guide has moved to the [GitHub Wiki](https://github.com/tenfyzhong/agentix/wiki/Backup-and-Recovery). Start with the [Wiki home](https://github.com/tenfyzhong/agentix/wiki/Home) for installation, configuration, and everyday use.

The implementation and development notes below remain in the repository.

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
