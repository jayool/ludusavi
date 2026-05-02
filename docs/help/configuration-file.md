# Configuration file

Ludusavi Sync stores its configuration in `config.yaml`, inside the
[application folder](application-folder.md).

You normally do not need to edit this file: the GUI updates it whenever
you change a setting. It is documented here mostly for reference and
troubleshooting.

## Example

```yaml
manifest:
  url: ~
roots:
  - path: "D:/Steam"
    store: steam
backup:
  path: ~/ludusavi-backup
restore:
  path: ~/ludusavi-backup
```
