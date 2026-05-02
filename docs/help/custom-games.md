# Custom games

The "custom games" screen lets you define your own save-data entries for
games that are not in the manifest, or override what the manifest says
about a game.

If the custom entry's name exactly matches a manifest game, your entry
takes precedence.

## File paths

You can type paths directly or use the browse button to pick a folder.
The path may also be a file (type the file name after browsing to its
parent folder), and it may use:

* [Globs](https://en.wikipedia.org/wiki/Glob_(programming)),
  for example `C:/example/*.txt`.
* The placeholders defined by the
  [Ludusavi Manifest format](https://github.com/mtkennerly/ludusavi-manifest)
  (`<base>`, `<home>`, `<storeUserId>`, etc.).

If a folder name itself contains a glob meta-character, escape it by
wrapping it in brackets (so `[` becomes `[[]`).

## Installed names

The "installed name" field tells Ludusavi Sync which subfolder of a root
the game lives in. It must be a bare folder name or relative path — never
absolute. The application also looks for the game's own title automatically,
so you only need this field when the install folder differs from the title.

For example, with an "other"-type root at `C:\Games`:

* If the game `Some Game` is installed at `C:\Games\sg`, set the installed
  name to `sg`.
* For a bundled game at `C:\Games\trilogy\first-game`, set it to
  `trilogy\first-game`.

## Save modes for custom games

Custom games support the same [save modes](save-modes.md) as manifest
games (`NONE`, `LOCAL`, `CLOUD`, `SYNC`). The default for a freshly
created custom game is `NONE`; change it from the main game list once the
custom entry has been saved.
