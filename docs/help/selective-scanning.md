# Selective scanning

After Ludusavi Sync has done at least one full scan (via the preview or
backup buttons), it remembers which games it found and shows them on
launch. From there you can act on a single game without re-scanning the
whole system.

The three-dot menu beside each game's title performs single-game actions.
You can also swap that menu for direct buttons by holding modifier keys
while hovering the game:

* `shift` — preview only.
* `ctrl` (or `cmd` on Mac) — backup or restore.
* `ctrl + alt` (or `cmd + option` on Mac) — backup or restore without a
  confirmation prompt.

The [save mode](save-modes.md) of each game also acts as an implicit
filter: games marked `NONE` are ignored by the daemon and by bulk backup
flows, so you can use the mode itself to keep noise out of the workflow.
