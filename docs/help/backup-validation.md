# Backup validation

The restore screen has a "validate" button that checks the integrity of the
latest backup for every game. You should not normally need to use this; it
exists for troubleshooting.

The check looks for:

* `mapping.yaml` files that are missing or malformed.
* Files that are listed in `mapping.yaml` but absent from the ZIP.

If problems are found, Ludusavi Sync will prompt you to take a fresh
backup of the affected games. Invalid backups are not deleted; that is up
to you.
