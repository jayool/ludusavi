# ![Logo](assets/icon.svg) Ludusavi Sync

Ludusavi Sync is a fork of [Ludusavi](https://github.com/mtkennerly/ludusavi)
focused on keeping your PC video game saves in sync across multiple devices
(for example, between a desktop PC and a Steam Deck).

It uses the same manifest data and detection engine as upstream Ludusavi,
but the user-facing flow is built around four explicit **save modes** per game
and an optional background **daemon** that picks up local changes automatically
and pushes/pulls them through your cloud remote.

## Differences from upstream

* GUI only. No command line, no shell completions, no `wrap`/launcher hooks.
* Single language (English). All translation/i18n machinery has been removed.
* No retention policies, no custom redirects, no backup filters,
  no per-game launch wrapping.
* Backups are always written as one ZIP per game (the only format).
* Each game has a **save mode** that controls how the daemon and the GUI handle it:
  `NONE`, `LOCAL`, `CLOUD`, or `SYNC`. See [save modes](docs/help/save-modes.md).
* Ships a separate `ludusavi-daemon` binary that watches your save folders
  and syncs them in the background. See [daemon](docs/help/daemon.md).
* Multi-device coordination uses a shared `game-list.json` in your cloud remote.
  See [multi-device](docs/help/multi-device.md).

## Features

* Detection for the games covered by the
  [Ludusavi Manifest](https://github.com/mtkennerly/ludusavi-manifest)
  (~19,000+ titles), plus your own custom entries.
* Steam, GOG, Epic, Heroic, Lutris, and other libraries via roots.
* File-based and (on Windows) registry-based saves.
* Proton saves on Linux via Steam.
* Cloud sync through [Rclone](https://rclone.org).

## Installation

Ludusavi Sync is portable. Drop the executable wherever you like.
See [installation](docs/help/installation.md) for the supported platforms
and any system-package prerequisites.

## Documentation

### Concepts
* [Save modes](docs/help/save-modes.md)
* [Daemon](docs/help/daemon.md)
* [Multi-device sync](docs/help/multi-device.md)
* [Cloud backup](docs/help/cloud-backup.md)
* [Roots](docs/help/roots.md)
* [Custom games](docs/help/custom-games.md)
* [Duplicates](docs/help/duplicates.md)
* [Selective scanning](docs/help/selective-scanning.md)

### Reference
* [Application folder](docs/help/application-folder.md)
* [Backup structure](docs/help/backup-structure.md)
* [Backup validation](docs/help/backup-validation.md)
* [Configuration file](docs/help/configuration-file.md)
* [Logging](docs/help/logging.md)

### Help
* [Missing saves](docs/help/missing-saves.md)
* [Troubleshooting](docs/help/troubleshooting.md)

## Credit

All credit for the core engine, manifest format, and detection logic goes to
[mtkennerly](https://github.com/mtkennerly) and the upstream Ludusavi project.
This fork only changes the workflow and removes features that were not needed
for the multi-device sync use case.
