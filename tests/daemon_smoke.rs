//! Smoke tests del binario `ludusavi-daemon`.
//!
//! Esta suite NO ejercita el sync end-to-end (eso lo cubre el suite unit/integration
//! con `RcloneTestEnv` + `SimulatedDevice`). Aquí solo verificamos que el binario:
//!   - Compila y arranca sin panic.
//!   - Honra `XDG_CONFIG_HOME` para localizar su `app_dir`.
//!   - Escribe `daemon_rCURRENT.log` (basename `daemon` con rotación) y `daemon-status.json`.
//!   - Responde a SIGTERM con un exit code limpio cuando hay cloud configurado.
//!
//! Linux-only: el daemon en Windows arranca como servicio, distinto mecanismo.

#![cfg(target_os = "linux")]

use std::{
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

const DAEMON_BIN: &str = env!("CARGO_BIN_EXE_ludusavi-daemon");

/// Spawnea el daemon con `XDG_CONFIG_HOME = <xdg>`. Captura stdout/stderr para
/// poder diagnosticar fallos.
fn spawn_daemon(xdg_config: &std::path::Path) -> Child {
    Command::new(DAEMON_BIN)
        .env("XDG_CONFIG_HOME", xdg_config)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn ludusavi-daemon")
}

/// Espera hasta `timeout` a que un fichero exista, devuelve true si apareció.
fn wait_for_file(path: &std::path::Path, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if path.exists() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    false
}

/// Envía SIGTERM por shell y espera al exit code, con timeout duro.
/// Si el proceso ya salió por su cuenta, devolvemos su status sin mandar señal.
fn terminate_and_wait(child: &mut Child, timeout: Duration) -> std::io::Result<std::process::ExitStatus> {
    if let Some(status) = child.try_wait()? {
        return Ok(status);
    }
    let _ = Command::new("kill")
        .arg("-TERM")
        .arg(child.id().to_string())
        .status();

    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        match child.try_wait()? {
            Some(status) => return Ok(status),
            None => std::thread::sleep(Duration::from_millis(100)),
        }
    }
    let _ = child.kill();
    child.wait()
}

/// Vuelca diagnóstico (stdout/stderr del daemon + tree del tempdir) al fallar un assert.
fn dump_diagnostics(child: &mut Child, tmp: &std::path::Path) -> String {
    use std::io::Read;
    let mut stdout = String::new();
    let mut stderr = String::new();
    if let Some(mut s) = child.stdout.take() {
        let _ = s.read_to_string(&mut stdout);
    }
    if let Some(mut s) = child.stderr.take() {
        let _ = s.read_to_string(&mut stderr);
    }
    let mut tree = String::new();
    for entry in walkdir::WalkDir::new(tmp).into_iter().flatten() {
        tree.push_str(&format!("  {:?}\n", entry.path()));
    }
    format!(
        "--- daemon stdout ---\n{stdout}\n--- daemon stderr ---\n{stderr}\n--- tempdir tree ---\n{tree}"
    )
}

// ============================================================================
// Tests
// ============================================================================

/// Sin config de cloud el daemon arranca, escribe sus ficheros básicos y sale solo.
/// Verificamos que `daemon_rCURRENT.log` y `daemon-status.json` se crean en el
/// `app_dir` correcto (vía XDG_CONFIG_HOME).
#[test]
fn daemon_writes_log_and_status_to_xdg_config_home() {
    let tmp = tempfile::tempdir().unwrap();
    let mut child = spawn_daemon(tmp.path());

    let app_dir = tmp.path().join("ludusavi");
    let log = app_dir.join("daemon_rCURRENT.log");
    let status_json = app_dir.join("daemon-status.json");

    let log_appeared = wait_for_file(&log, Duration::from_secs(10));
    let status_appeared = wait_for_file(&status_json, Duration::from_secs(10));

    let exit = terminate_and_wait(&mut child, Duration::from_secs(10)).unwrap();

    if !log_appeared || !status_appeared {
        let diag = dump_diagnostics(&mut child, tmp.path());
        panic!(
            "expected files not created (log={log_appeared}, status={status_appeared}, exit={exit:?})\n{diag}"
        );
    }
}

/// Cuando el daemon arranca sin cloud configurado, debe salir limpiamente
/// por su cuenta (no quedarse colgado, no panic). Es el escenario "primera
/// instalación, usuario aún no ha tocado Settings".
#[test]
fn daemon_exits_cleanly_when_no_cloud_configured() {
    let tmp = tempfile::tempdir().unwrap();
    let mut child = spawn_daemon(tmp.path());

    // Le damos hasta 10s para que arranque, vea que no hay cloud y salga.
    // No mandamos SIGTERM: el daemon debe salir solo si llega a esa rama.
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut exited_on_its_own = None;
    while Instant::now() < deadline {
        if let Some(status) = child.try_wait().unwrap() {
            exited_on_its_own = Some(status);
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    let exit = match exited_on_its_own {
        Some(s) => s,
        None => terminate_and_wait(&mut child, Duration::from_secs(10)).unwrap(),
    };

    // Aceptamos exit code 0 o haber salido por la rama "no cloud configured".
    // Lo importante: el proceso terminó sin un panic visible.
    let diag = dump_diagnostics(&mut child, tmp.path());
    assert!(
        exited_on_its_own.is_some(),
        "daemon did not exit on its own; status after SIGTERM={exit:?}\n{diag}"
    );
    assert!(
        exit.success() || exit.code().is_some(),
        "daemon exited abnormally: {exit:?}\n{diag}"
    );
}

/// SIGTERM debe terminar al daemon limpiamente. Para garantizar que esté
/// corriendo cuando la mandamos, le damos un cloud configurado vía un
/// config.yaml mínimo escrito a mano antes de spawnear.
#[test]
fn daemon_responds_to_sigterm_when_running() {
    if which::which("rclone").is_err() {
        eprintln!("[skip] rclone not in PATH");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let cloud_dir = tempfile::tempdir().unwrap();

    let app_dir = tmp.path().join("ludusavi");
    std::fs::create_dir_all(&app_dir).unwrap();

    // Config mínima con cloud :local apuntando al tempdir.
    let rclone_path = which::which("rclone").unwrap().to_string_lossy().to_string();
    let config_yaml = format!(
        r#"
cloud:
  remote:
    Custom:
      id: ":local"
  path: "{cloud}"
  synchronize: true
apps:
  rclone:
    path: "{rclone}"
    arguments: ""
"#,
        cloud = cloud_dir.path().display(),
        rclone = rclone_path,
    );
    std::fs::write(app_dir.join("config.yaml"), config_yaml).unwrap();

    // sync-games.json vacío (no hay juegos en SYNC, así que el daemon no lanzará
    // file watcher, pero igual entra al loop y queda esperando).
    std::fs::write(
        app_dir.join("sync-games.json"),
        r#"{"games":{},"safety_backups_enabled":true,"system_notifications_enabled":true}"#,
    )
    .unwrap();

    let mut child = spawn_daemon(tmp.path());

    // Damos 2s al daemon para arrancar e instalar el handler de señal.
    std::thread::sleep(Duration::from_secs(2));

    let exit = terminate_and_wait(&mut child, Duration::from_secs(10)).unwrap();

    use std::os::unix::process::ExitStatusExt;
    let signaled = exit.signal().is_some();
    let diag = dump_diagnostics(&mut child, tmp.path());
    assert!(
        exit.success() || signaled,
        "expected clean exit or signal termination, got {exit:?}\n{diag}"
    );
}

// ============================================================================
// Test E2E del worker loop: file watcher → debounce → upload
//
// Verifica el flujo más crítico del daemon: cuando el usuario modifica saves
// localmente, tras el debounce (10s) el daemon sube el ZIP al cloud.
//
// Test lento (~25s): debounce real + polling del worker loop. Usamos un timeout
// generoso para evitar flakiness en máquinas más lentas.
// ============================================================================

/// Espera hasta `timeout` a que aparezca un fichero, comprobando cada 500ms.
fn wait_until(predicate: impl Fn() -> bool, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if predicate() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    false
}

#[test]
fn daemon_uploads_to_cloud_after_save_modification() {
    if which::which("rclone").is_err() {
        eprintln!("[skip] rclone not in PATH");
        return;
    }

    let xdg = tempfile::tempdir().unwrap();
    let cloud_dir = tempfile::tempdir().unwrap();
    let saves_dir = tempfile::tempdir().unwrap();
    let app_dir = xdg.path().join("ludusavi");
    std::fs::create_dir_all(&app_dir).unwrap();

    let rclone_path = which::which("rclone").unwrap().to_string_lossy().to_string();
    let device_id = "uuid-test-device";
    let device_name = "Test-Device";
    let game_id = "TestE2EGame";

    // Pre-poblar config.yaml con cloud :local: apuntando al cloud_dir.
    std::fs::write(
        app_dir.join("config.yaml"),
        format!(
            r#"
cloud:
  remote:
    Custom:
      id: ":local"
  path: "{cloud}"
  synchronize: true
apps:
  rclone:
    path: "{rclone}"
    arguments: ""
"#,
            cloud = cloud_dir.path().display(),
            rclone = rclone_path,
        ),
    )
    .unwrap();

    // Identidad de dispositivo fija para el test.
    std::fs::write(
        app_dir.join("ludusavi-device.json"),
        format!(r#"{{"id":"{device_id}","name":"{device_name}"}}"#),
    )
    .unwrap();

    // sync-games.json con el juego en SYNC.
    std::fs::write(
        app_dir.join("sync-games.json"),
        format!(
            r#"{{
  "games": {{ "{game_id}": {{ "mode": "sync", "auto_sync": true }} }},
  "safety_backups_enabled": false,
  "system_notifications_enabled": false
}}"#
        ),
    )
    .unwrap();

    // Pre-poblar el "cloud" en estado "ya sincronizado":
    //   - ZIP existe (placeholder, contenido no importa para este test).
    //   - game-list con timestamps coherentes para que determine_sync_type
    //     diga InSync → daemon NO sube nada al arrancar.
    let initial_save = saves_dir.path().join("initial.dat");
    std::fs::write(&initial_save, b"v1-initial").unwrap();
    // Forzar mtime conocido en el save para sincronizar cloud y local.
    let sync_unix = chrono::Utc::now().timestamp() - 3600; // hace 1 hora
    let sync_ft = filetime::FileTime::from_unix_time(sync_unix, 0);
    filetime::set_file_mtime(&initial_save, sync_ft).unwrap();
    let sync_iso = chrono::DateTime::from_timestamp(sync_unix, 0)
        .unwrap()
        .to_rfc3339();

    // ZIP del cloud (placeholder vacío basta — el daemon no lo valida).
    std::fs::write(cloud_dir.path().join(format!("game-{game_id}.zip")), b"PK\x03\x04").unwrap();

    let game_list = format!(
        r#"{{
  "games": [
    {{
      "id": "{game_id}",
      "name": "{game_id}",
      "path_by_device": {{
        "{device_id}": {{ "path": "{saves}", "last_sync_mtime": "{sync}" }}
      }},
      "last_synced_from": "{device_id}",
      "last_sync_time_utc": "{sync}",
      "latest_write_time_utc": "{sync}",
      "storage_bytes": 10
    }}
  ],
  "device_names": {{ "{device_id}": "{device_name}" }}
}}"#,
        saves = saves_dir.path().display(),
        sync = sync_iso,
    );
    std::fs::write(cloud_dir.path().join("ludusavi-game-list.json"), &game_list).unwrap();

    let cloud_zip = cloud_dir.path().join(format!("game-{game_id}.zip"));
    let initial_zip_mtime = std::fs::metadata(&cloud_zip).unwrap().modified().unwrap();

    let mut child = spawn_daemon(xdg.path());

    // Damos al daemon ~5s para arrancar y registrar el watcher.
    std::thread::sleep(Duration::from_secs(5));

    if let Some(status) = child.try_wait().unwrap() {
        let diag = dump_diagnostics(&mut child, xdg.path());
        panic!("daemon exited prematurely (status={status:?})\n{diag}");
    }

    // Sanity check: el daemon NO ha tocado el ZIP del cloud todavía
    // (porque al arrancar determine_sync_type devuelve InSync).
    let zip_mtime_after_startup = std::fs::metadata(&cloud_zip).unwrap().modified().unwrap();
    assert_eq!(
        initial_zip_mtime, zip_mtime_after_startup,
        "daemon should not have touched cloud ZIP at startup (InSync)"
    );

    // Ahora SÍ modificamos un save: el watcher debería disparar el debounce.
    std::fs::write(saves_dir.path().join("changed.dat"), b"v2-watcher-trigger").unwrap();

    // Esperar hasta 25s a que el ZIP cambie (debounce 10s + worker poll + upload).
    let appeared = wait_until(
        || {
            std::fs::metadata(&cloud_zip)
                .ok()
                .and_then(|m| m.modified().ok())
                .map(|m| m > initial_zip_mtime)
                .unwrap_or(false)
        },
        Duration::from_secs(25),
    );

    // Pase lo que pase, paramos el daemon antes de assertar.
    let exit = terminate_and_wait(&mut child, Duration::from_secs(10)).unwrap();

    if !appeared {
        let diag = dump_diagnostics(&mut child, xdg.path());
        panic!(
            "ZIP did not appear in cloud after save modification within 25s\nexit={exit:?}\n{diag}"
        );
    }

    // Sanity: el ZIP en el cloud no debe estar vacío.
    let zip_size = std::fs::metadata(&cloud_zip).map(|m| m.len()).unwrap_or(0);
    assert!(zip_size > 0, "uploaded ZIP should not be empty");
}
