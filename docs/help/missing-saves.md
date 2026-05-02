# What if my saves aren't found?

Ludusavi Sync gets its detection data from the
[Ludusavi Manifest](https://github.com/mtkennerly/ludusavi-manifest),
which is generated from [PCGamingWiki](https://www.pcgamingwiki.com).

When a game's saves are not detected, the order of things to check is:

1. **Does the game have a PCGamingWiki article with save info?**
   Open the wiki page and check the `Game data` section for save and
   config locations matching your platform (Windows vs Linux vs Mac,
   Steam vs Epic, and so on). If the data is missing or wrong, fixing
   the wiki is the long-term solution — your fix will eventually flow
   into the manifest.

2. **Has the manifest been updated since the wiki was?**
   The manifest is regenerated from the wiki every few hours. If a save
   location was added very recently it may not be in the manifest yet.
   The "other" screen shows when the manifest was last downloaded.

3. **Are your roots configured correctly?**
   Some save locations can only be discovered with a particular kind of
   root. For example, Ludusavi Sync can only check Steam's `compatdata`
   folder if you have configured a Steam root. See [roots](roots.md).

4. **Steam Cloud fallback.** When a game has a wiki article but no save
   info, Ludusavi Sync also checks Steam Cloud metadata as a fallback.
   This only helps for games with Steam Cloud support; once the wiki
   itself lists save locations, the Steam Cloud info is ignored.

5. **Linux/Proton.** When the wiki has Windows save locations but no
   Linux/Mac locations, Ludusavi Sync derives some likely paths (such
   as Steam's `compatdata/<app id>/`) automatically. Edge cases — e.g.
   non-Steam games launched through Proton — sometimes need wiki fixes.
