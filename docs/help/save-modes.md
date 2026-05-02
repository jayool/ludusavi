# Save modes

Every game in Ludusavi Sync has a **save mode** that tells the application
how to handle that game's data. This is the central concept that drives the
GUI, the daemon, and multi-device sync.

There are four modes:

## `NONE`

Default for newly detected games. The game is known to Ludusavi Sync but is
not actively managed: the daemon ignores it, and it does not appear in
backup/restore flows unless you explicitly select it.

Use this for games you have not played in a while or do not care about syncing.

## `LOCAL`

The game is backed up to your local backup folder, but never uploaded to the
cloud. Use this for games whose saves are too large to be worth uploading,
or for single-device games where you only want a local safety net.

## `CLOUD`

The game is backed up to your local backup folder **and** synchronised with
the cloud remote. The cloud copy is the source of truth across devices. Use
this for games you might play on more than one machine and want to be able
to pick up from where you left off.

## `SYNC`

Same as `CLOUD`, but the daemon will also automatically push changes shortly
after it detects that the save files on disk have been modified. Use this
for games you actively play and want to keep in sync without thinking about
it.

## Picking a mode

A reasonable starting point:

* `NONE` for games you do not currently play.
* `LOCAL` for very large saves you do not want to upload.
* `CLOUD` for games you play on a single device but might restore later.
* `SYNC` for games you play across two or more devices.

You can change a game's mode at any time from the main game list.

## How modes interact with the daemon

The [daemon](daemon.md) only acts on games whose mode is `SYNC`. `LOCAL`
and `CLOUD` games are still backed up and (for `CLOUD`) synced when you
trigger a backup manually from the GUI, but the daemon will not touch them
on its own.

## How modes are stored

Each game's mode is part of the configuration file (`config.yaml`) and is
also recorded in the cloud-side `game-list.json` so that other devices
sharing the same remote can see it. See [multi-device](multi-device.md).
