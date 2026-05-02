# Application folder

Ludusavi Sync stores its configuration, logs, and cache in the following
locations:

* Windows: `%APPDATA%/ludusavi`
* Linux: `$XDG_CONFIG_HOME/ludusavi` or `~/.config/ludusavi`
* Mac: `~/Library/Application Support/ludusavi`

If you would prefer the application to keep its configuration next to the
executable (for example, when running from a flash drive), create an empty
file called `ludusavi.portable` in the same directory as the executable.

The application folder also contains `manifest.yaml` (the game-detection
data). You should not edit it by hand: Ludusavi Sync overwrites it whenever
it downloads a fresh copy.
