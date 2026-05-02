# Backup structure

Inside your backup folder, Ludusavi Sync creates one subfolder per game.
The folder name is derived from the game's title; characters that are not
valid on the filesystem are replaced with `_`. In rare cases (where every
character would be invalid), the folder is named
`ludusavi-renamed-<ENCODED_NAME>`.

Each game folder contains:

* `mapping.yaml` — metadata that Ludusavi Sync uses to identify the game
  during restore. Without this file, the folder is ignored on restore.
* A single ZIP file with the game's saved files.
* On Windows, if the game has registry-based saves, a `registry.yaml` file
  inside the ZIP. On Linux with Steam + Proton, the relevant `*.reg` files
  are backed up alongside the regular files.

Ludusavi Sync only writes ZIP-format backups; the legacy "simple" layout
that earlier versions of upstream Ludusavi supported has been removed.
The ZIP preserves the original modification time of each file.

## Absolute paths

Backups record the absolute path of each save file (for example,
`C:\Users\foo\save.dat`) rather than a relative or placeholder path. This
is the safest behaviour when restoring on the same system, but it means
that restoring a backup taken on a different system or under a different
username may need you to move files into place by hand. Cross-OS transfer
is discussed in [multi-device](multi-device.md).
