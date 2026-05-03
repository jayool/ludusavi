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
import os
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

    # ACCELA's utils.helpers.resource_path() locates bundled binaries
    # (DepotDownloader.dll, Goldberg/, Steamless/, etc.) by looking at
    # sys._MEIPASS first (PyInstaller convention) and falling back to
    # the directory of sys.argv[0]. In the source distribution, deps/
    # lives at bin/src/deps/ (next to main.py), not at bin/deps/. When
    # ACCELA runs normally via `python src/main.py`, sys.argv[0]'s
    # directory is bin/src/ and resource_path resolves correctly. Since
    # we run from the adapter folder, we point _MEIPASS at bin/src so
    # resource_path("deps/...") returns bin/src/deps/...
    sys._MEIPASS = str(src_dir)

    # ACCELA stores settings under QSettings("Tachibana Labs", "ACCELA").
    # We initialise a headless QCoreApplication with the same names so
    # QSettings resolves to the same store the user configured via the GUI.
    from PyQt6.QtCore import QCoreApplication

    app = QCoreApplication.instance()
    if app is None:
        app = QCoreApplication(sys.argv[:1])
    app.setOrganizationName("Tachibana Labs")
    app.setApplicationName("ACCELA")

    _install_qthread_compat_patches()


def _install_qthread_compat_patches() -> None:
    """Make ACCELA's QThread + TaskRunner code run synchronously on the
    main thread inside our adapter.

    ACCELA dispatches long-running tasks (DownloadDepotsTask was one,
    SteamlessTask, ApplicationShortcutsTask, GenerateAchievementsTask)
    by spawning a QThread and waiting on a nested QEventLoop. In the
    standalone ACCELA CLI this works because that process has a full
    QApplication driving its main event loop. In our headless adapter
    we only have a QCoreApplication, and queued signals from a worker
    QThread to lambdas connected on the main thread don't get
    delivered through a nested QEventLoop — the loop blocks but the
    cross-thread signal queue is never drained.

    Workaround: redirect TaskRunner.run() and SteamlessTask.start() to
    schedule the work via QTimer.singleShot(0, ...) on the main thread.
    The nested QEventLoop processes the timer event, runs the task
    synchronously on the main thread, and signals fire as
    DirectConnection (same thread = synchronous slot call), so the
    lambdas execute as expected.
    """
    import traceback

    from PyQt6.QtCore import QObject, Qt, pyqtSignal
    from utils.task_runner import TaskRunner, Worker
    from core.tasks.steamless_task import SteamlessTask

    # Helper QObject whose `fire` signal we use to dispatch a no-arg
    # callable into the main thread's event queue with QueuedConnection.
    # QTimer.singleShot(0, ...) does NOT fire inside our nested QEventLoop
    # (likely because we only have QCoreApplication, not QApplication,
    # and singleShot's internal timer dispatcher gets bypassed by nested
    # loops). A QueuedConnection signal does post a regular event onto
    # the thread's queue, which loop.exec() drains.
    class _Dispatcher(QObject):
        fire = pyqtSignal()

    # Keep dispatchers alive so the queued connections aren't dropped.
    _dispatchers: list = []

    def _post_to_main_thread(callable_):
        d = _Dispatcher()
        d.fire.connect(callable_, Qt.ConnectionType.QueuedConnection)
        _dispatchers.append(d)
        d.fire.emit()

    def sync_run(self, target_func, *args, **kwargs):
        self.worker = Worker(target_func, *args, **kwargs)

        def do_work():
            try:
                result = target_func(*args, **kwargs)
                self.worker.finished.emit(result)
            except Exception as e:
                self.worker.error.emit((type(e), e, traceback.format_exc()))
            finally:
                self.worker.completed.emit()
                self.cleanup_complete.emit()

        TaskRunner._active_runners.append(self)
        _post_to_main_thread(do_work)
        return self.worker

    TaskRunner.run = sync_run

    def sync_steamless_start(self):
        def do_run():
            try:
                self.run()
            finally:
                # QThread.finished is normally emitted when the worker
                # thread exits; since we're not using a real thread
                # here, emit it manually so loop.quit() callbacks fire.
                self.finished.emit()

        _post_to_main_thread(do_run)

    SteamlessTask.start = sync_steamless_start


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


def handle_process_zip(payload: Dict[str, Any]) -> None:
    """Parse a manifest ZIP (from morrenus or dropped by the user) and emit
    the depot data inside it. Synchronous and Qt-free at the task level."""
    from core.tasks.process_zip_task import ProcessZipTask

    zip_path = payload.get("path")
    if not zip_path:
        emit_error("process_zip: 'path' is required")
        return

    try:
        game_data = ProcessZipTask().run(zip_path)
    except Exception as e:
        emit_error(f"process_zip: {e!r}")
        return

    emit(
        {
            "event": "depots_parsed",
            "appid": game_data.get("appid"),
            "game_name": game_data.get("game_name"),
            "depots": game_data.get("depots", {}),
            "dlcs": game_data.get("dlcs", {}),
            "manifests": game_data.get("manifests", {}),
            "header_url": game_data.get("header_url"),
            "installdir": game_data.get("installdir"),
            "buildid": game_data.get("buildid"),
        }
    )


# Spec of ACCELA settings exposed to Ludusavi's UI. Any key not in this spec
# is hidden from get_settings; set_setting accepts any key but the UI will not
# round-trip values not declared here.
SETTINGS_SPEC: Dict[str, Any] = {
    # Downloads tab
    "library_mode": ("bool", False),
    "auto_skip_single_choice": ("bool", False),
    "max_downloads": ("int", 255),
    "generate_achievements": ("bool", False),
    "use_steamless": ("bool", False),
    "auto_apply_goldberg": ("bool", False),
    "create_application_shortcuts": ("bool", False),
    # Integrations tab
    "morrenus_api_key": ("str", ""),
    "sgdb_api_key": ("str", ""),
    # Steam tab
    "slssteam_mode": ("bool", False),
    "sls_config_management": ("bool", True),
    "prompt_steam_restart": ("bool", True),
    "block_steam_updates": ("bool", False),
}


def _coerce(value: Any, kind: str, default: Any) -> Any:
    if value is None:
        return default
    if kind == "bool":
        if isinstance(value, bool):
            return value
        if isinstance(value, (int, float)):
            return bool(value)
        if isinstance(value, str):
            return value.strip().lower() in ("true", "1", "yes", "on")
        return default
    if kind == "int":
        try:
            return int(value)
        except (TypeError, ValueError):
            return default
    return str(value)


def handle_get_settings(_payload: Dict[str, Any]) -> None:
    from PyQt6.QtCore import QSettings

    settings = QSettings("Tachibana Labs", "ACCELA")
    values: Dict[str, Any] = {}
    for key, (kind, default) in SETTINGS_SPEC.items():
        raw = settings.value(key, default)
        values[key] = _coerce(raw, kind, default)
    emit({"event": "settings", "values": values})


def handle_set_setting(payload: Dict[str, Any]) -> None:
    from PyQt6.QtCore import QSettings

    key = payload.get("key")
    if not key:
        emit_error("set_setting: 'key' is required")
        return
    if "value" not in payload:
        emit_error("set_setting: 'value' is required")
        return
    settings = QSettings("Tachibana Labs", "ACCELA")
    settings.setValue(key, payload["value"])
    settings.sync()
    emit({"event": "setting_saved", "key": key})


def handle_get_morrenus_stats(_payload: Dict[str, Any]) -> None:
    from core.morrenus_api import get_user_stats

    stats = get_user_stats()
    if isinstance(stats, dict) and stats.get("error"):
        emit_error(f"get_morrenus_stats: {stats['error']}")
        return
    emit({"event": "morrenus_stats", "stats": stats})


def handle_apply_steam_updates_block(payload: Dict[str, Any]) -> None:
    """Toggle Steam's auto-update block by writing/removing steam.cfg in the
    Steam install directory. The QSetting itself is set separately via
    set_setting; this handler only handles the file side-effect."""
    import os
    import shutil

    enabled = bool(payload.get("enabled", False))
    try:
        from core.steam_helpers import find_steam_install
        from utils.paths import Paths
    except Exception as e:
        emit_error(f"apply_steam_updates_block: import failed: {e!r}")
        return

    try:
        path = find_steam_install()
    except Exception as e:
        emit_error(f"apply_steam_updates_block: find_steam_install failed: {e!r}")
        return

    if not path:
        emit({"event": "tool_done", "kind": "apply_steam_updates_block", "note": "Steam install not found; skipped."})
        return

    dest = os.path.join(path, "steam.cfg")
    src = Paths.deps("steam.cfg")

    try:
        if enabled:
            if not src.exists():
                emit_error("apply_steam_updates_block: bundled steam.cfg missing")
                return
            shutil.copy2(str(src), dest)
            emit({"event": "tool_done", "kind": "apply_steam_updates_block", "note": f"Wrote {dest}"})
        else:
            if os.path.exists(dest):
                os.remove(dest)
                emit({"event": "tool_done", "kind": "apply_steam_updates_block", "note": f"Removed {dest}"})
            else:
                emit({"event": "tool_done", "kind": "apply_steam_updates_block", "note": "No steam.cfg to remove."})
    except (OSError, IOError) as e:
        emit_error(f"apply_steam_updates_block: {e!r}")


def _manage_registry_inline(filename: str) -> str:
    """Replicate ACCELA's SettingsDialog._manage_registry without Qt UI calls.
    Returns a status note for the tool_done event. Windows only."""
    import os
    import subprocess
    import sys
    import tempfile

    if sys.platform != "win32":
        raise RuntimeError("registry actions are Windows-only")

    base = os.path.join(
        os.path.dirname(os.path.abspath(sys.argv[0])),
        "deps",
    ) if not getattr(sys, "frozen", False) else os.path.join(getattr(sys, "_MEIPASS"), "deps")

    # Adapter is run from outside ACCELA, so sys.argv[0] is adapter.py.
    # Resolve the deps folder relative to ACCELA's src dir which is on sys.path.
    if not os.path.exists(os.path.join(base, filename)):
        for entry in sys.path:
            candidate = os.path.join(os.path.dirname(entry), "deps", filename)
            if os.path.exists(candidate):
                base = os.path.dirname(candidate)
                break

    reg_path = os.path.join(base, filename)
    if not os.path.exists(reg_path):
        raise FileNotFoundError(f"Missing {filename} (looked in {base})")

    with open(reg_path, "r", encoding="utf-8-sig") as f:
        content = f.read().replace("[INSTALL_PATH]", sys.executable.replace("\\", "\\\\"))

    with tempfile.NamedTemporaryFile(mode="w", suffix=".reg", delete=False) as tmp:
        tmp.write(content)
        tmp_name = tmp.name

    try:
        subprocess.run(["regedit", "/s", tmp_name], check=True, shell=True)
    finally:
        try:
            os.unlink(tmp_name)
        except OSError:
            pass

    return f"Imported {filename}"


def handle_download_depots(payload: Dict[str, Any]) -> None:
    """Run DownloadDepotsTask with the user's depot selection, stream
    progress events to stdout, then run CLITaskManager post-processing.

    Calls download_task.run() directly on the main thread. The task does
    its own subprocess management internally; wrapping it in a QThread
    (as ACCELA's CLI does) caused signals emitted from the worker thread
    to never reach the lambdas connected on the main thread, leaving the
    GUI stuck at 0% with an empty log.
    """
    from core.tasks.download_depots_task import DownloadDepotsTask
    from utils.settings import get_settings
    from managers.cli_manager import CLITaskManager


    game_data = payload.get("game_data")
    selected_depots = payload.get("depots") or []
    dest_path = payload.get("dest")

    if not isinstance(game_data, dict):
        emit_error("download_depots: 'game_data' (object) is required")
        return
    if not selected_depots:
        emit_error("download_depots: 'depots' (non-empty list) is required")
        return
    if not dest_path:
        emit_error("download_depots: 'dest' (string) is required")
        return

    # Wrap str() depot ids — selection from the GUI may come as ints.
    selected_depots = [str(d) for d in selected_depots]
    game_data["selected_depots_list"] = selected_depots

    # Logger-like object that emits each call as a JSON progress event.
    class JsonLogger:
        def __init__(self, phase: str):
            self.phase = phase

        def info(self, msg):
            emit({"event": "progress", "phase": self.phase, "message": str(msg)})

        def warning(self, msg):
            emit({"event": "progress", "phase": self.phase, "message": f"WARNING: {msg}"})

        def error(self, msg):
            emit({"event": "progress", "phase": self.phase, "message": f"ERROR: {msg}"})

        def critical(self, msg):
            emit({"event": "progress", "phase": self.phase, "message": f"CRITICAL: {msg}"})

        def debug(self, _msg):
            pass  # too noisy for the GUI

        def exception(self, msg):
            emit({"event": "progress", "phase": self.phase, "message": f"EXCEPTION: {msg}"})

    postprocess_logger = JsonLogger("postprocess")

    download_task = DownloadDepotsTask()
    download_task.progress.connect(
        lambda msg: emit({"event": "progress", "phase": "download", "message": msg})
    )
    download_task.progress_percentage.connect(
        lambda pct: emit(
            {"event": "progress", "phase": "download", "percentage": int(pct)}
        )
    )

    try:
        download_task.run(game_data, selected_depots, dest_path)
    except Exception as e:
        emit_error(f"download_depots: {e!r}")
        return

    emit(
        {
            "event": "progress",
            "phase": "postprocess",
            "message": "Download phase complete. Starting post-processing...",
        }
    )

    try:
        settings = get_settings()
        task_manager = CLITaskManager(settings, postprocess_logger)
        task_manager.run_post_processing(game_data, download_task, dest_path)
    except Exception as e:
        emit_error(f"post_processing: {e!r}")
        return

    emit(
        {
            "event": "download_done",
            "game_name": game_data.get("game_name"),
            "appid": game_data.get("appid"),
            "dest": dest_path,
        }
    )


def handle_get_steam_libraries(_payload: Dict[str, Any]) -> None:
    """Return the list of detected Steam library paths so the GUI can offer
    them as preset destinations (in addition to the freeform folder picker)."""
    try:
        from core.steam_helpers import get_steam_libraries
    except Exception as e:
        emit_error(f"get_steam_libraries: import failed: {e!r}")
        return
    try:
        libs = get_steam_libraries()
    except Exception as e:
        emit_error(f"get_steam_libraries: {e!r}")
        return
    emit({"event": "steam_libraries", "libraries": list(libs) if libs else []})


def handle_run_tool(payload: Dict[str, Any]) -> None:
    """Run a Tools-tab action. kind selects which one."""
    kind = payload.get("kind")
    if not kind:
        emit_error("run_tool: 'kind' is required")
        return

    try:
        if kind == "register_protocol":
            note = _manage_registry_inline("ACCELA.reg")
            emit({"event": "tool_done", "kind": kind, "note": note})
            return
        if kind == "unregister_protocol":
            note = _manage_registry_inline("ACCELA_uninstall.reg")
            emit({"event": "tool_done", "kind": kind, "note": note})
            return
        if kind == "run_slscheevo":
            from utils.helpers import get_slscheevo_path, get_slscheevo_save_path, get_venv_python
            import os
            import sys

            path = get_slscheevo_path()
            if not os.path.exists(path):
                emit_error(f"run_tool: SLScheevo missing at {path}")
                return
            save = get_slscheevo_save_path()
            cmd = []
            if str(path).endswith(".py"):
                py = get_venv_python()
                cmd.append(py if py else ("python" if sys.platform == "win32" else "python3"))
            cmd.extend([str(path), "--save-dir", str(save), "--noclear", "--max-tries", "101"])
            _launch_in_terminal(cmd, os.path.dirname(path))
            emit({"event": "tool_done", "kind": kind, "note": "Launched SLScheevo in a terminal."})
            return
        if kind == "run_steamless":
            exe = payload.get("exe_path")
            if not exe:
                emit_error("run_tool: 'exe_path' is required for run_steamless")
                return
            from core.tasks.steamless_task import SteamlessTask

            task = SteamlessTask()
            task.set_game_directory(exe)
            task.start()
            # Steamless runs synchronously inside its own thread; we just kick it off.
            emit({"event": "tool_done", "kind": kind, "note": f"Steamless started on {exe}"})
            return
        emit_error(f"run_tool: unknown kind '{kind}'")
    except Exception as e:
        emit_error(f"run_tool {kind}: {e!r}")


def _launch_in_terminal(cmd: list, cwd: str) -> None:
    """Try to launch a command in a visible terminal. Cross-platform."""
    import subprocess
    import sys

    cmd = [str(part) for part in cmd]
    cwd = str(cwd)

    if sys.platform == "win32":
        q = " ".join([f'"{c}"' if " " in c else c for c in cmd])
        subprocess.Popen(f'start cmd /k "cd /d {cwd} && {q}"', shell=True)
        return

    terms = [
        ["wezterm", "start", "--always-new-process", "--"],
        ["konsole", "-e"],
        ["gnome-terminal", "--"],
        ["alacritty", "-e"],
        ["xterm", "-e"],
    ]
    import shutil

    for term in terms:
        if shutil.which(term[0]):
            try:
                subprocess.Popen(term + cmd, cwd=cwd)
                return
            except FileNotFoundError:
                continue
    raise RuntimeError("No terminal emulator found")


HANDLERS = {
    "search": handle_search,
    "fetch_manifest": handle_fetch_manifest,
    "process_zip": handle_process_zip,
    "get_settings": handle_get_settings,
    "set_setting": handle_set_setting,
    "get_morrenus_stats": handle_get_morrenus_stats,
    "apply_steam_updates_block": handle_apply_steam_updates_block,
    "run_tool": handle_run_tool,
    "download_depots": handle_download_depots,
    "get_steam_libraries": handle_get_steam_libraries,
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
