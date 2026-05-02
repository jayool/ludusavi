# Roots

Roots are folders where Ludusavi Sync looks for installed games and their
save data. On first launch, the application tries to detect a few common
roots automatically; you can review and adjust them on the "other" screen.

Each root has a **type** that tells the application what to expect inside.

## Root types

* **Steam.** The folder containing `steamapps` and `userdata`.
  Common locations: `C:/Program Files (x86)/Steam` on Windows,
  `~/.steam/steam` on Linux. On Linux, Proton's `*.reg` files are
  backed up automatically for games known to have registry-based saves.
  Non-Steam games added to Steam are also picked up if their shortcut
  name in Steam matches the title used by the manifest.

* **Heroic.** The folder containing `gog_store` and `GamesConfig`.
  Heroic stores GOG, Epic, Amazon, and sideloaded games here, and on
  Linux also exposes Wine, Proton, and Lutris prefixes that
  Ludusavi Sync can scan.

* **Legendary.** The folder containing `installed.json`. Wine prefixes
  for Legendary roots are not currently detected.

* **Lutris.** The folder containing the `games` subdirectory. Each
  game's YAML must have at least `name` and either `game.working_dir`
  or `game.exe`; games missing those fields are skipped.

* **"Other" and remaining store-specific types.** A folder whose direct
  children are individual games. For example, in the Epic Games Store
  this is the install location you chose for your library
  (e.g. `D:/Epic`, with `D:/Epic/Celeste` underneath).

* **Home folder.** Any folder that should also be treated as a `~`
  equivalent. Useful when you have changed `HOME` to relocate save data.

* **Wine prefix.** A folder containing `drive_c`. File-based saves are
  backed up; registry-based saves inside the prefix are not.

* **Windows / Linux / Mac drive.** External drives carrying a separate
  OS install. For example, an old Windows drive turned into an external
  USB drive can be added as a Windows drive root so its saves can be
  scanned. In this mode, only the default locations of system folders
  are checked — there is no access to the OS APIs or `XDG` variables
  that would resolve relocated folders.

## Globs

You may use [globs](https://en.wikipedia.org/wiki/Glob_(programming))
in root paths to match several folders at once. Escape literal glob
meta-characters by wrapping them in brackets (`[` becomes `[[]`).

## Order

The order in which roots are listed does not matter. The only edge case
is secondary manifests (`.ludusavi.yaml` files): if two of them define
overlapping data for the same game, Ludusavi Sync merges them in the
order it discovered them.
