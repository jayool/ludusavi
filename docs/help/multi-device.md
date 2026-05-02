# Multi-device sync

Ludusavi Sync is designed around the idea that you have a primary device
(for example, a desktop PC) and one or more secondary devices (for example,
a Steam Deck or a laptop), and you want save files to follow you between
them.

## How it works

1. All devices are configured to use the **same cloud remote** through Rclone.
   See [cloud backup](cloud-backup.md).
2. Inside that remote, Ludusavi Sync maintains a shared file called
   `game-list.json`. It lists every game that has been marked as `CLOUD` or
   `SYNC` on any device, along with that game's save mode and the most
   recent backup timestamp.
3. When you start the GUI on a new device, it reads `game-list.json` to
   learn which games are managed by the fleet, downloads their backups on
   demand, and lets you restore them locally.
4. When you finish a play session, the [daemon](daemon.md) (for `SYNC`
   games) or a manual backup (for `CLOUD` games) pushes the new save back
   to the remote, updating `game-list.json` so the other devices know there
   is something newer to download.

## Initial setup on a new device

1. Install Ludusavi Sync.
2. Configure your roots so the application can find your installed games.
   See [roots](roots.md).
3. Configure the same Rclone remote you use on your primary device.
   See [cloud backup](cloud-backup.md).
4. Open the GUI; it will pull `game-list.json` and show you the games that
   are currently managed.

## Conflicts

If two devices both make changes to the same game's save while offline,
the second one to upload will see a conflict warning when it tries to push.
You can resolve the conflict by choosing which copy to keep (local or
cloud) from the relevant screen. Ludusavi Sync does not try to merge save
files automatically.

## Notes

* Cross-OS save transfer (for example, Windows save → Linux save for the
  same game) is **not** automatic. Some games store data in completely
  different places or formats on different operating systems. Ludusavi Sync
  pushes and pulls the bytes; it does not translate between OS layouts.
* If your primary and secondary devices are both Windows (or both Linux),
  most games will sync transparently. Mixed setups (PC + Deck) work best
  for games whose save format is OS-independent or for games run through
  Proton on the Linux side.
