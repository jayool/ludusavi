# ACCELA adapter

Headless bridge between Ludusavi Sync (Rust/Iced) and ACCELA (Python/PyQt6).
Imports ACCELA's modules and exposes a JSON-lines protocol over stdin/stdout
so Ludusavi Sync can drive ACCELA without showing ACCELA's GUI.

Used by Ludusavi Sync to render the ACCELA tab natively in Iced while
delegating the actual logic (manifest fetch, depot processing, download,
post-processing) to ACCELA itself.

## Status

**Phase 0** — search and fetch_manifest only. No download, no post-processing
yet. See *Roadmap* below.

## Requirements

* ACCELA installed and reachable on disk. The adapter is pointed at
  ACCELA's `bin/` folder via `--accela-path`.
* Same Python interpreter that ACCELA uses, with ACCELA's dependencies
  available (PyQt6, requests, etc.). On Linux this is typically
  `<accela>/bin/.venv/bin/python`. On Windows it's whatever Python the
  user installed ACCELA's `requirements.txt` into.
* The user's morrenus API key already configured inside ACCELA (the
  adapter reads it from the same QSettings store that ACCELA's GUI writes
  to).

## Protocol

JSON-lines over stdin/stdout. Each message is one complete JSON object
on its own line. Stdout is flushed on every event.

### Commands (Ludusavi → adapter)

```json
{"cmd":"search","query":"portal","limit":50}
{"cmd":"fetch_manifest","appid":"400"}
```

`limit` is optional (defaults to 100, capped at 100 by morrenus).

### Events (adapter → Ludusavi)

```json
{"event":"search_results","games":[{"appid":400,"name":"Portal", ...}],"total_count":1}
{"event":"manifest_ready","zip":"/home/jayoo/.local/share/ACCELA/morrenus_manifests/accela_fetch_400.zip","appid":"400"}
{"event":"error","message":"search: API Key is not set..."}
```

### Error handling

* Invalid JSON on a stdin line → `error` event, loop continues.
* Unknown `cmd` → `error` event, loop continues.
* Bootstrap failure (bad `--accela-path`, PyQt6 missing, etc.) → `error`
  event then exit code 1.
* Any exception inside a handler → `error` event with `repr(e)`, loop
  continues.

The adapter exits cleanly when stdin closes (EOF).

## How to test by hand

These commands assume you've already run ACCELA at least once so the venv
exists and your morrenus API key is saved.

### Linux

```bash
ACCELA_BIN=/path/to/ACCELA-…-source/bin
PY="$ACCELA_BIN/.venv/bin/python"

# Search
echo '{"cmd":"search","query":"portal","limit":3}' \
    | "$PY" accela_adapter/adapter.py --accela-path "$ACCELA_BIN"

# Fetch manifest by AppID
echo '{"cmd":"fetch_manifest","appid":"400"}' \
    | "$PY" accela_adapter/adapter.py --accela-path "$ACCELA_BIN"

# Several commands in one session
printf '%s\n' \
    '{"cmd":"search","query":"half-life"}' \
    '{"cmd":"fetch_manifest","appid":"70"}' \
    | "$PY" accela_adapter/adapter.py --accela-path "$ACCELA_BIN"
```

### Windows (PowerShell)

```powershell
$AccelaBin = "C:\path\to\ACCELA-...-source\bin"
$Py = "$AccelaBin\.venv\Scripts\python.exe"  # or your system Python

# Search
'{"cmd":"search","query":"portal","limit":3}' `
    | & $Py .\accela_adapter\adapter.py --accela-path $AccelaBin
```

### What you should see

A successful search emits one `search_results` event then waits for the
next stdin line (or exits if stdin is closed). A successful
`fetch_manifest` emits one `manifest_ready` event with the absolute path
of the saved ZIP — that ZIP lives under ACCELA's data folder
(`~/.local/share/ACCELA/morrenus_manifests/` on Linux,
`%APPDATA%\ACCELA\morrenus_manifests\` on Windows).

## Roadmap (later phases)

| Phase | Adds |
|------:|------|
| 1 | Iced side: ACCELA tab in sidebar, search box, results list. No new adapter code. |
| 2 | Adapter: `process_zip` + `download` commands with `progress` events. |
| 3 | Iced side: depot selection modal + progress bar. |
| 4 | Adapter: `postprocess` command with per-step `progress` events. |
| 5 | Iced side: post-processing progress display. |
| 6 | Auto-rescan of Ludusavi Sync roots when a download finishes. |
| 7 | Settings: configurable ACCELA path / Python interpreter / .NET path. |

## Why this lives in the same repo

The adapter is intentionally co-located with Ludusavi Sync rather than
shipped as a separate package. That keeps the protocol versioned together
with the Iced code that consumes it: when one side changes, the other
side is in the same commit. The cost is that ACCELA must be installed
separately (this repo does not bundle ACCELA itself).
