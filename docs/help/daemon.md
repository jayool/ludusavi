# Daemon

Ludusavi Sync ships a separate background process, `ludusavi-daemon`, whose
job is to keep games marked as `SYNC` (see [save modes](save-modes.md))
synchronised without you having to open the GUI.

## What it does

The daemon:

1. Loads the same `config.yaml` that the GUI uses.
2. Watches the on-disk save folders of every game whose mode is `SYNC`.
3. When it detects that a save file has changed and has been quiet for a few
   seconds, it triggers a backup for that game.
4. Pushes the resulting ZIP to the configured cloud remote.

It does **not** touch games whose mode is `NONE`, `LOCAL`, or `CLOUD`. Those
modes are still serviced by the GUI on demand.

## Installing as a service

The daemon binary is built alongside the GUI and is intended to be run as a
user-level background service.

* **Windows:** can be registered as a Windows service using
  `ludusavi-daemon` itself (see the in-binary install command).
* **Linux:** typically run as a `systemd` user service.

The exact install/uninstall steps depend on your platform; both binaries are
self-contained executables that you can also run directly from a terminal
to test before installing as a service.

## Logs

The daemon writes to the same [application folder](application-folder.md)
as the GUI and uses the same [logging](logging.md) configuration. Log lines
from the daemon are tagged so you can tell them apart from the GUI's lines.

## When the daemon is not running

If the daemon is stopped, nothing breaks. The GUI continues to work as
usual, and you can perform backups and cloud sync manually from the
relevant screens. Restarting the daemon will pick up any changes that
happened while it was off the next time the corresponding files change.
