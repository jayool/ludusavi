#!/usr/bin/env python3
"""
ACCELA adapter for Ludusavi Sync.

Bridges Ludusavi Sync (Rust/Iced) and ACCELA (Python/PyQt6) by importing
ACCELA's modules and exposing a JSON-lines protocol over stdin/stdout.

The adapter runs headless: it does not show any GUI. Ludusavi Sync sends
commands as JSON lines on stdin and receives events as JSON lines on stdout.

Phase 0 scope:
    - Commands: search, fetch_manifest
    - Events:   search_results, manifest_ready, error

Future phases will add: process_zip, download, postprocess.

Usage:
    python adapter.py --accela-path /path/to/ACCELA/bin

Where the path is the bin/ folder of ACCELA (the one containing src/ and
run.sh). The adapter reads ACCELA's settings (including the user's morrenus
API key) from the same QSettings store that the GUI uses.
"""

import argparse
import json
import sys
from pathlib import Path
from typing import Any, Dict


def emit(event: Dict[str, Any]) -> None:
    """Write one JSON event to stdout, newline-terminated, flushed."""
    sys.stdout.write(json.dumps(event) + "\n")
    sys.stdout.flush()


def emit_error(message: str) -> None:
    emit({"event": "error", "message": message})


def bootstrap(accela_path: Path) -> None:
    """Prepare sys.path and Qt so ACCELA modules can be imported and used."""
    src_dir = accela_path / "src"
    if not src_dir.is_dir():
        raise FileNotFoundError(f"ACCELA src/ not found at {src_dir}")

    sys.path.insert(0, str(src_dir))

    # ACCELA stores settings under QSettings("Tachibana Labs", "ACCELA").
    # We initialise a headless QCoreApplication with the same names so
    # QSettings resolves to the same store the user configured via the GUI.
    from PyQt6.QtCore import QCoreApplication

    app = QCoreApplication.instance()
    if app is None:
        app = QCoreApplication(sys.argv[:1])
    app.setOrganizationName("Tachibana Labs")
    app.setApplicationName("ACCELA")


def handle_search(payload: Dict[str, Any]) -> None:
    from core.morrenus_api import search_games

    query = payload.get("query", "")
    limit = payload.get("limit", 100)

    if not query:
        emit_error("search: 'query' is required")
        return

    result = search_games(query, limit)

    if isinstance(result, dict) and "error" in result:
        emit_error(f"search: {result['error']}")
        return

    games = result.get("results", []) if isinstance(result, dict) else []
    total = (
        result.get("total_count", len(games))
        if isinstance(result, dict)
        else len(games)
    )

    emit({"event": "search_results", "games": games, "total_count": total})


def handle_fetch_manifest(payload: Dict[str, Any]) -> None:
    from core.morrenus_api import download_manifest

    appid = payload.get("appid")
    if appid is None or appid == "":
        emit_error("fetch_manifest: 'appid' is required")
        return

    zip_path, error = download_manifest(str(appid))

    if error:
        emit_error(f"fetch_manifest: {error}")
        return

    emit({"event": "manifest_ready", "zip": zip_path, "appid": str(appid)})


HANDLERS = {
    "search": handle_search,
    "fetch_manifest": handle_fetch_manifest,
}


def main_loop() -> int:
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue

        try:
            msg = json.loads(line)
        except json.JSONDecodeError as e:
            emit_error(f"invalid JSON: {e}")
            continue

        cmd = msg.get("cmd")
        if not cmd:
            emit_error("missing 'cmd' field")
            continue

        handler = HANDLERS.get(cmd)
        if handler is None:
            emit_error(f"unknown command: {cmd}")
            continue

        try:
            handler(msg)
        except Exception as e:
            emit_error(f"{cmd} failed: {e!r}")

    return 0


def main() -> int:
    parser = argparse.ArgumentParser(
        description="ACCELA adapter for Ludusavi Sync (headless JSON-lines bridge)"
    )
    parser.add_argument(
        "--accela-path",
        required=True,
        type=Path,
        help="Path to ACCELA's bin/ folder (the one containing src/ and run.sh)",
    )
    args = parser.parse_args()

    try:
        bootstrap(args.accela_path)
    except Exception as e:
        emit_error(f"bootstrap failed: {e!r}")
        return 1

    return main_loop()


if __name__ == "__main__":
    sys.exit(main())
