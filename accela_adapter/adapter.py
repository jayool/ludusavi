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
    """Replace ACCELA's QThread-based post-processing methods with
    synchronous equivalents that run on the main thread.

    Background: ACCELA dispatches long-running tasks (Steamless,
    ApplicationShortcutsTask, GenerateAchievementsTask) by spawning a
    QThread and waiting on a nested QEventLoop for the worker's
    finished signal. In the standalone ACCELA CLI this works because
    that process has a full QApplication driving its main event loop.

    In our headless adapter we only have a QCoreApplication, and
    queued signals across threads (QThread → main-thread lambdas)
    never get delivered when only a nested QEventLoop is active. Even
    explicitly forcing QueuedConnection or scheduling via QTimer.
    singleShot does not work — the nested loop blocks but the
    cross-thread/event queue never drains.

    Direct fix: replace _run_steamless / _run_application_shortcuts /
    _run_achievement_generation on CLITaskManager with synchronous
    versions that just call task.run() directly on the main thread.
    Signals emitted during run() use DirectConnection (same thread =
    synchronous slot call), so the lambdas reach our JsonLogger and
    we get progress events on stdout in real time.
    """
    import os
    import sys as _sys

    from managers.cli_manager import CLITaskManager
    from utils.steam_manifest import get_game_directory
    from utils.yaml_config_manager import is_slssteam_mode_enabled

    def sync_run_steamless(self):
        """Synchronous replacement for CLITaskManager._run_steamless."""
        if not self.current_dest_path or not self.game_data:
            return
        game_directory = get_game_directory(self.current_dest_path, self.game_data)
        if not os.path.exists(game_directory):
            return

        from core.tasks.steamless_task import SteamlessTask

        self.logger.info("Starting Steamless DRM Removal...")
        self.logger.info(f"Processing directory: {game_directory}")

        steamless_task = SteamlessTask()
        steamless_task.progress.connect(self.logger.info)
        steamless_task.set_game_directory(game_directory)
        steamless_task.run()  # direct, on main thread; signals are sync

        self.logger.info("Steamless processing completed")

    def sync_run_application_shortcuts(self):
        """Synchronous replacement for CLITaskManager._run_application_shortcuts."""
        if not self.game_data:
            return
        if _sys.platform != "linux":
            self.logger.info("Application shortcuts are only supported on Linux")
            return
        if not is_slssteam_mode_enabled():
            self.logger.info(
                "Steam library integration is disabled, skipping shortcuts creation"
            )
            return
        app_id = self.game_data.get("appid")
        game_name = self.game_data.get("game_name")
        if not app_id:
            return
        sgdb_api_key = self.settings.value("sgdb_api_key", "", type=str)
        if not sgdb_api_key:
            return

        try:
            from core.tasks.application_shortcuts import ApplicationShortcutsTask
        except ImportError:
            self.logger.error("ApplicationShortcutsTask module not found")
            return

        self.logger.info("Creating application shortcuts...")
        shortcuts_task = ApplicationShortcutsTask()
        shortcuts_task.set_api_key(sgdb_api_key)
        shortcuts_task.progress.connect(self.logger.info)
        shortcuts_task.run(app_id, game_name)  # direct
        self.logger.info("Application shortcuts created")

    def sync_run_achievement_generation(self):
        """Synchronous replacement for CLITaskManager._run_achievement_generation."""
        if not self.game_data:
            return
        app_id = self.game_data.get("appid")
        if not app_id:
            return

        from core.tasks.generate_achievements_task import GenerateAchievementsTask

        self.logger.info("Generating achievements...")
        achievement_task = GenerateAchievementsTask()
        achievement_task.progress.connect(self.logger.info)
        result = achievement_task.run(app_id)  # direct
        if result and result.get("success"):
            self.logger.info(
                f"Achievement generation completed: {result.get('message')}"
            )
        else:
            self.logger.info(
                f"Achievement generation failed: "
                f"{result.get('message') if result else 'Unknown error'}"
            )

    CLITaskManager._run_steamless = sync_run_steamless
    CLITaskManager._run_application_shortcuts = sync_run_application_shortcuts
    CLITaskManager._run_achievement_generation = sync_run_achievement_generation

    # Workaround for an ACCELA bug: cli_manager.py:_create_applist_file
    # calls find_next_applist_number(app_list_dir, self.logger) with two
    # args, but core.steam_helpers.find_next_applist_number(app_list_dir)
    # only accepts one. Replace the method with a corrected version.
    from core.steam_helpers import (
        app_id_exists_in_applist,
        find_next_applist_number,
    )

    def fixed_create_applist_file(self, app_list_dir, appid, is_dlc=False):
        if not app_id_exists_in_applist(app_list_dir, appid):
            next_num = find_next_applist_number(app_list_dir)
            filepath = os.path.join(app_list_dir, f"{next_num}.txt")
            with open(filepath, "w", encoding="utf-8") as f:
                f.write(str(appid))
            log_msg = f"Created GreenLuma file: {filepath} for "
            log_msg += f"DLC: {appid}" if is_dlc else f"AppID: {appid}"
            self.logger.info(log_msg)
        else:
            log_msg = (
                f"AppID {appid} already exists in AppList folder. "
                f"Skipping file creation."
            )
            if is_dlc:
                log_msg = f"DLC {log_msg}"
            self.logger.info(log_msg)

    CLITaskManager._create_applist_file = fixed_create_applist_file


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


def handle_restart_steam(_payload: Dict[str, Any]) -> None:
    """Kill and relaunch Steam, mirroring ACCELA's
    ``JobQueueManager._perform_steam_restart`` (job_queue_manager.py:207-273)
    minus the GUI dialogs. Runs synchronously on this thread; the caller
    shows progress in its own UI.

    Emits a single ``steam_restarted`` event with ``ok: true|false`` and an
    optional ``note`` describing the path taken or the failure reason.
    """
    import os
    import sys
    import time

    try:
        from core import steam_helpers
    except Exception as e:
        emit({"event": "steam_restarted", "ok": False, "note": f"import failed: {e!r}"})
        return

    try:
        if sys.platform == "linux":
            steam_helpers.kill_steam_process()
            time.sleep(1)
            result = steam_helpers.start_steam()
            if result == "SUCCESS":
                emit({"event": "steam_restarted", "ok": True, "note": "Steam restarted."})
                return
            if result == "NEEDS_USER_PATH":
                # ACCELA falls back to a GUI dialog asking the user to
                # locate Steam. We don't have a GUI here, so surface the
                # condition and let Iced show the message.
                emit({
                    "event": "steam_restarted",
                    "ok": False,
                    "note": "Steam path could not be auto-detected.",
                })
                return
            emit({"event": "steam_restarted", "ok": False, "note": "Failed to start Steam."})
            return

        # Windows path: prefer DLLInjector.exe if present, fall back to
        # plain steam.exe launch when user32.dll wrapper is in place.
        steam_path = steam_helpers.find_steam_install()
        if not steam_path:
            emit({
                "event": "steam_restarted",
                "ok": False,
                "note": "Could not find Steam installation path.",
            })
            return

        steam_helpers.kill_steam_process()
        time.sleep(1)

        injector_path = os.path.join(steam_path, "DLLInjector.exe")
        if os.path.exists(injector_path):
            # DLLInjector requires admin rights (its manifest is marked
            # ``requireAdministrator``). Plain ``subprocess.Popen`` would
            # fail with WinError 740 because we inherit Ludusavi's
            # non-elevated token. Use ShellExecuteW with the ``runas``
            # verb to trigger a UAC prompt for this child only.
            import ctypes
            import ctypes.wintypes as wt

            shell32 = ctypes.windll.shell32
            shell32.ShellExecuteW.argtypes = [
                wt.HWND,
                wt.LPCWSTR,
                wt.LPCWSTR,
                wt.LPCWSTR,
                wt.LPCWSTR,
                ctypes.c_int,
            ]
            shell32.ShellExecuteW.restype = ctypes.c_void_p

            SW_SHOWNORMAL = 1
            ret = shell32.ShellExecuteW(
                None, "runas", injector_path, None, steam_path, SW_SHOWNORMAL
            )
            # Success when the return value is > 32. <= 32 is a SE_ERR_*
            # error code; UAC denial commonly surfaces as 5 (access
            # denied) or 1223 (cancelled).
            ret_int = int(ret) if ret is not None else 0
            if ret_int > 32:
                emit({
                    "event": "steam_restarted",
                    "ok": True,
                    "note": "Steam restarted via DLLInjector (UAC accepted).",
                })
            else:
                if ret_int in (5, 1223):
                    note = "DLLInjector launch was cancelled (UAC declined)."
                elif ret_int == 2:
                    note = "DLLInjector.exe not found at runtime."
                else:
                    note = (
                        f"Could not launch DLLInjector.exe (ShellExecute error {ret_int}). "
                        "Check that SLSsteam is installed correctly."
                    )
                emit({"event": "steam_restarted", "ok": False, "note": note})
            return

        user32_path = os.path.join(steam_path, "user32.dll")
        if os.path.exists(user32_path):
            steam_helpers.start_steam()
            emit({
                "event": "steam_restarted",
                "ok": True,
                "note": "Steam restarted (user32.dll wrapper detected).",
            })
            return

        # Vanilla Steam: no SLSsteam wrappers detected. Just launch steam.exe.
        if steam_helpers.start_steam() == "SUCCESS":
            emit({"event": "steam_restarted", "ok": True, "note": "Steam restarted."})
        else:
            emit({
                "event": "steam_restarted",
                "ok": False,
                "note": "Could not launch steam.exe.",
            })
    except Exception as e:
        emit({"event": "steam_restarted", "ok": False, "note": f"{e!r}"})


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


def _accela_marker_path(game_path: str) -> str:
    """Return the absolute path of the ACCELA install marker if present.

    Mirrors ``GameManager._get_accela_marker_path`` (game_manager.py:447).
    Either ``.ACCELA`` or ``.DepotDownloader`` counts as a marker.
    """
    for name in (".ACCELA", ".DepotDownloader"):
        candidate = os.path.join(game_path, name)
        if os.path.exists(candidate):
            return candidate
    return ""


def _has_real_game_content(game_path: str) -> bool:
    """True iff the directory has files beyond the ACCELA markers / OS junk.
    Mirrors ``GameManager._has_game_content`` (game_manager.py:415)."""
    ignore = {".accela", ".depotdownloader", "desktop.ini", "thumbs.db"}
    try:
        with os.scandir(game_path) as entries:
            for entry in entries:
                try:
                    name = entry.name
                    if name.lower() in ignore or name.startswith("."):
                        continue
                    if entry.is_file() or entry.is_dir():
                        return True
                except (OSError, FileNotFoundError, PermissionError):
                    continue
    except OSError:
        return False
    return False


def _find_acf_for_game(library_path: str, game_name: str):
    """Locate the ``appmanifest_*.acf`` whose ``installdir`` matches game_name.

    Returns ``(appmanifest_path, appid)`` or ``(None, None)``. Mirrors
    ``GameManager._parse_acf_for_appid`` (game_manager.py:713).
    """
    import re

    steamapps = os.path.join(library_path, "steamapps")
    if not os.path.exists(steamapps):
        return None, None
    try:
        with os.scandir(steamapps) as entries:
            for entry in entries:
                try:
                    if not (
                        entry.name.startswith("appmanifest_")
                        and entry.name.endswith(".acf")
                    ):
                        continue
                    try:
                        with open(entry.path, "r", encoding="utf-8") as f:
                            content = f.read()
                    except (OSError, IOError, PermissionError):
                        continue
                    m = re.search(r'"installdir"\s+"([^"]+)"', content)
                    if not m:
                        continue
                    installdir = m.group(1)
                    if installdir.lower() == game_name.lower():
                        appid = entry.name.replace("appmanifest_", "").replace(".acf", "")
                        return entry.path, appid
                except (OSError, FileNotFoundError, PermissionError):
                    continue
    except OSError:
        pass
    return None, None


def _parse_acf_metadata(appmanifest_path: str) -> Dict[str, Any]:
    """Extract ``buildid``, ``LastUpdated``, ``SizeOnDisk``, ``name`` from an ACF.
    Mirrors ``GameManager._parse_acf_for_metadata`` (game_manager.py:780)."""
    import re

    out: Dict[str, Any] = {
        "name": "",
        "buildid": "",
        "last_updated": "",
        "size_on_disk": 0,
    }
    try:
        with open(appmanifest_path, "r", encoding="utf-8") as f:
            content = f.read()
    except (OSError, IOError, PermissionError):
        return out

    for key, target in (
        ("name", "name"),
        ("buildid", "buildid"),
        ("LastUpdated", "last_updated"),
    ):
        m = re.search(rf'"{key}"\s+"([^"]+)"', content)
        if m:
            out[target] = m.group(1)
    m = re.search(r'"SizeOnDisk"\s+"([^"]+)"', content)
    if m:
        try:
            size = int(m.group(1))
            if size > 0:
                out["size_on_disk"] = size
        except ValueError:
            pass
    return out


def handle_list_accela_installs(_payload: Dict[str, Any]) -> None:
    """Scan all Steam libraries for games installed via ACCELA.

    A game is "installed via ACCELA" if its directory under
    ``steamapps/common/`` contains a ``.ACCELA`` or ``.DepotDownloader``
    marker folder (created by DepotDownloader during a Phase 4 download).

    Vanilla Steam installs are deliberately excluded: Ludusavi exposes
    these entries as a fourth source for the Games table, and only the
    ACCELA-managed ones offer meaningful Overview/Uninstall/Tools
    actions in our integration.
    """
    try:
        from core.steam_helpers import get_steam_libraries
    except Exception as e:
        emit_error(f"list_accela_installs: import failed: {e!r}")
        return

    libraries = get_steam_libraries() or []
    games = []

    for library in libraries:
        common = os.path.join(library, "steamapps", "common")
        if not os.path.exists(common):
            continue
        try:
            with os.scandir(common) as entries:
                for entry in entries:
                    try:
                        if not entry.is_dir():
                            continue
                        marker = _accela_marker_path(entry.path)
                        if not marker:
                            continue
                        if not _has_real_game_content(entry.path):
                            continue

                        appmanifest_path, appid = _find_acf_for_game(library, entry.name)
                        meta = _parse_acf_metadata(appmanifest_path) if appmanifest_path else {}

                        games.append({
                            "appid": appid or "0",
                            "game_name": meta.get("name") or entry.name,
                            "install_dir": entry.name,
                            "install_path": entry.path,
                            "library_path": library,
                            "size_on_disk": meta.get("size_on_disk", 0),
                            "buildid": meta.get("buildid", ""),
                            "last_updated": meta.get("last_updated", ""),
                            "source": "ACCELA",
                            "is_accela_install": True,
                            "accela_marker_path": marker,
                            "appmanifest_path": appmanifest_path or "",
                        })
                    except (OSError, FileNotFoundError, PermissionError):
                        continue
        except OSError:
            continue

    emit({"event": "accela_installs", "games": games})


def handle_uninstall_game(payload: Dict[str, Any]) -> None:
    """Uninstall a game: rmtree the install dir, delete its ACF, optionally
    purge Linux-only state (Proton compatdata, cloud saves).

    Mirrors the Linux/Windows branches of ``GameManager.uninstall_game``
    (game_manager.py:1004) but headless (no Qt dialogs).
    """
    import shutil
    import sys as _sys

    install_path = payload.get("install_path") or ""
    appmanifest_path = payload.get("appmanifest_path") or ""
    library_path = payload.get("library_path") or ""
    appid = payload.get("appid") or ""
    remove_compatdata = bool(payload.get("remove_compatdata", False))
    remove_saves = bool(payload.get("remove_saves", False))

    if not install_path:
        emit_error("uninstall_game: 'install_path' is required")
        return

    errors = []

    if os.path.exists(install_path):
        try:
            shutil.rmtree(install_path)
        except OSError as e:
            errors.append(f"rmtree install dir: {e!r}")

    if appmanifest_path and os.path.exists(appmanifest_path):
        try:
            os.remove(appmanifest_path)
        except OSError as e:
            errors.append(f"remove ACF: {e!r}")

    if _sys.platform == "linux" and library_path and appid:
        if remove_compatdata:
            compat = os.path.join(library_path, "steamapps", "compatdata", appid)
            if os.path.exists(compat):
                try:
                    shutil.rmtree(compat)
                except OSError as e:
                    errors.append(f"rmtree compatdata: {e!r}")
        if remove_saves:
            userdata = os.path.join(library_path, "userdata")
            if os.path.exists(userdata):
                try:
                    with os.scandir(userdata) as users:
                        for user in users:
                            if not user.is_dir():
                                continue
                            remote = os.path.join(user.path, appid, "remote")
                            if os.path.exists(remote):
                                try:
                                    shutil.rmtree(remote)
                                except OSError as e:
                                    errors.append(f"rmtree saves {user.name}: {e!r}")
                except OSError as e:
                    errors.append(f"scan userdata: {e!r}")

    emit({
        "event": "uninstall_done",
        "ok": len(errors) == 0,
        "errors": errors,
    })


def handle_fix_install(payload: Dict[str, Any]) -> None:
    """Delete only the ``appmanifest_*.acf`` so Steam stops thinking the
    game is installed (without touching the game files themselves).

    Useful when a Steam DB sync after an ACCELA install accidentally
    re-registers the game and Steam tries to download it on top.
    """
    appmanifest_path = payload.get("appmanifest_path")
    if not appmanifest_path:
        emit_error("fix_install: 'appmanifest_path' is required")
        return
    if not os.path.exists(appmanifest_path):
        emit({"event": "fix_install_done", "ok": True, "note": "ACF was already absent."})
        return
    try:
        os.remove(appmanifest_path)
        emit({"event": "fix_install_done", "ok": True, "note": "ACF deleted."})
    except OSError as e:
        emit({"event": "fix_install_done", "ok": False, "note": f"{e!r}"})


def _stub_task_manager():
    """Instantiate ``TaskManager`` with a minimal main_window stub.

    ``TaskManager.__init__`` only needs ``main_window.settings``; the rest
    of the references to ``self.main_window`` in the methods we call are
    gated behind ``show_dialog`` (always passed as ``False`` from us).
    """
    from PyQt6.QtCore import QSettings
    from managers.task_manager import TaskManager

    class _MainWindowStub:
        def __init__(self):
            self.settings = QSettings("Tachibana Labs", "ACCELA")

    return TaskManager(_MainWindowStub())


def handle_apply_goldberg(payload: Dict[str, Any]) -> None:
    """Apply the Goldberg Steam emulator to a game directory via
    ``TaskManager.apply_goldberg_to_game`` (task_manager.py:779).
    """
    install_path = payload.get("install_path")
    appid = payload.get("appid")
    game_name = payload.get("game_name", "")
    if not install_path:
        emit_error("apply_goldberg: 'install_path' is required")
        return
    if not appid:
        emit_error("apply_goldberg: 'appid' is required")
        return
    try:
        tm = _stub_task_manager()
        ok = tm.apply_goldberg_to_game(install_path, appid, game_name, show_dialog=False)
        emit({"event": "goldberg_done", "ok": bool(ok)})
    except Exception as e:
        emit({"event": "goldberg_done", "ok": False, "note": f"{e!r}"})


def handle_remove_goldberg(payload: Dict[str, Any]) -> None:
    """Restore the original ``steam_api*.dll`` backups via
    ``TaskManager.remove_goldberg_from_game`` (task_manager.py:824).
    """
    install_path = payload.get("install_path")
    appid = payload.get("appid", "")
    game_name = payload.get("game_name", "")
    if not install_path:
        emit_error("remove_goldberg: 'install_path' is required")
        return
    try:
        tm = _stub_task_manager()
        ok = tm.remove_goldberg_from_game(install_path, appid, game_name, show_dialog=False)
        emit({"event": "goldberg_removed", "ok": bool(ok)})
    except Exception as e:
        emit({"event": "goldberg_removed", "ok": False, "note": f"{e!r}"})


def handle_run_steamless_for_game(payload: Dict[str, Any]) -> None:
    """Run Steamless on every executable Steamless considers in the game
    directory. ``SteamlessTask.set_game_directory`` walks the dir and
    queues exes; ``run`` (sync entrypoint) processes them sequentially.
    """
    install_path = payload.get("install_path")
    if not install_path:
        emit_error("run_steamless_for_game: 'install_path' is required")
        return
    try:
        from core.tasks.steamless_task import SteamlessTask

        task = SteamlessTask()
        task.set_game_directory(install_path)
        # Synchronous run, same approach as our Phase 4 monkey-patch of
        # CLITaskManager._run_steamless. Avoids QThread/QEventLoop which
        # do not work without a Qt event loop.
        task.run()
        emit({
            "event": "steamless_done",
            "ok": True,
            "note": f"Steamless ran on {install_path}.",
        })
    except Exception as e:
        emit({"event": "steamless_done", "ok": False, "note": f"{e!r}"})


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
    "restart_steam": handle_restart_steam,
    "list_accela_installs": handle_list_accela_installs,
    "uninstall_game": handle_uninstall_game,
    "fix_install": handle_fix_install,
    "apply_goldberg": handle_apply_goldberg,
    "remove_goldberg": handle_remove_goldberg,
    "run_steamless_for_game": handle_run_steamless_for_game,
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
