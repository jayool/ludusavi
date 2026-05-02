# Troubleshooting

## GUI

* **Window content overflows the screen on Linux.** Set the
  `WINIT_X11_SCALE_FACTOR` environment variable to `1`.

* **GUI won't launch.**
    * The graphics drivers may not be cooperating. Force the software
      renderer with `ICED_BACKEND=tiny-skia`.
    * Try forcing the dedicated GPU rather than the integrated one. On
      Windows: Settings → System → Display → Graphics. Cross-platform:
      set `WGPU_POWER_PREF=high`.
    * Try a different graphics backend with
      `WGPU_BACKEND=dx12` (or `vulkan`, `metal`, `gl`).

* **A console window stays open behind the GUI on Windows 11.**
  This is a known limitation of Windows Terminal
  ([microsoft/terminal#14416](https://github.com/microsoft/terminal/issues/14416)).
  Workaround: open Windows Terminal, go to its settings, and switch
  "default terminal application" to "Windows Console Host".

* **Steam Deck file picker doesn't work.** Use desktop mode instead of
  game mode.

## Backups and restores

* **Long paths don't back up on Windows.** Ludusavi Sync supports long
  paths, but Windows itself must enable them too. See
  [Microsoft's instructions](https://learn.microsoft.com/en-us/windows/win32/fileio/maximum-file-path-limitation?tabs=registry#registry-setting-to-enable-long-paths).

* **Restoring fails with `Access is denied. (os error 5)`.**
    * On Windows, the application cannot create new folders directly
      inside `C:/Program Files`. If a launcher or game lives there,
      reinstall the launcher first so its parent folder exists, then
      retry the restore.
    * On Windows, the application cannot create new folders directly
      inside `C:/Users` either. If your backup was taken under a
      different username, you will need to copy the files into the new
      user folder by hand before restoring.
    * In general: make sure the account running Ludusavi Sync has write
      permission on the target folder.

## Setting environment variables on Windows

If you need to set one of the environment variables mentioned above on
Windows:

* Open the Start Menu, search for `edit the system environment
  variables`, and pick the matching result.
* Click the `environment variables...` button.
* In the upper `user variables` section, click `new...` (or select an
  existing variable and click `edit...`), and enter the name and value.
