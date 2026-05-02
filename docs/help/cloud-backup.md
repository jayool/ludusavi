# Cloud backup

Ludusavi Sync uses [Rclone](https://rclone.org) for cloud storage. You
configure the remote on the "other" screen.

Any Rclone remote will work. The application can guide you through setting
up the most common ones (Google Drive, OneDrive, Dropbox, Box, FTP, SMB,
WebDAV); for anything else, configure the remote with `rclone config` first
and then point Ludusavi Sync at the existing remote name.

## How it ties into save modes

Cloud sync is what makes the difference between save modes:

* `LOCAL` games are never sent to the cloud.
* `CLOUD` games are sent on demand whenever you trigger a backup.
* `SYNC` games are sent automatically by the [daemon](daemon.md) shortly
  after the on-disk save changes.

If your local and cloud copies are already in sync at the start of a
backup, any new changes are uploaded once the backup completes. If they
are out of sync, Ludusavi Sync warns you about the conflict and leaves
the cloud data untouched. Manual upload and download buttons on the
"other" screen let you resolve the conflict.

## Tuning Rclone

Cloud throughput depends on your network, the cloud provider, and Rclone
itself. The "other" screen has a field for custom Rclone arguments.
Useful flags include:

* `--fast-list` and/or `--ignore-checksum` to speed transfers up.
* `--transfers=1` to avoid being rate-limited at the cost of speed.

Full list: https://rclone.org/flags

## Alternative: filesystem-mounted clouds

You don't strictly need Ludusavi Sync's built-in Rclone integration. If
you have a tool that exposes your cloud storage as a normal folder, you
can point the backup target at that folder instead:

* [Google Drive for Desktop](https://www.google.com/drive/download) creates
  a `G:` drive that streams from/to the cloud.
* [Syncthing](https://syncthing.net) keeps a local folder mirrored across
  devices.
* `rclone mount` mounts any remote as a folder.

In all of these cases, the daemon's `SYNC` behaviour still works: it sees
file changes locally and triggers a backup, and the underlying tool then
moves the bytes.
