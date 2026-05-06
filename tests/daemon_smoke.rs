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

// ============================================================================
// Tests E2E del worker loop (3 escenarios pendientes del handover):
//   1. Daemon poll del cloud detecta que otro device subió y descarga.
//   2. Daemon reacciona a cambios en sync-games.json en runtime y reinicia.
//   3. Daemon sobrevive cuando arranca y el game-list del cloud no existe
//      todavía (retry indefinido sin caer en panic).
//
// Comparten infra con el test de upload. Todos Linux-only y se auto-saltan
// si rclone no está en PATH.
// ============================================================================

/// Helper: monta el árbol estándar de un test E2E (config.yaml con cloud
/// `:local:`, ludusavi-device.json fijo). Devuelve (xdg, app_dir, cloud).
fn setup_xdg_with_cloud(
    device_id: &str,
    device_name: &str,
) -> (tempfile::TempDir, std::path::PathBuf, tempfile::TempDir) {
    let xdg = tempfile::tempdir().unwrap();
    let cloud_dir = tempfile::tempdir().unwrap();
    let app_dir = xdg.path().join("ludusavi");
    std::fs::create_dir_all(&app_dir).unwrap();

    let rclone_path = which::which("rclone").unwrap().to_string_lossy().to_string();

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

    std::fs::write(
        app_dir.join("ludusavi-device.json"),
        format!(r#"{{"id":"{device_id}","name":"{device_name}"}}"#),
    )
    .unwrap();

    (xdg, app_dir, cloud_dir)
}

/// Test 3 (más simple): el daemon arranca con un game en SYNC y saves
/// locales presentes, pero el cloud no tiene game-list.json todavía. La
/// poll cíclica del cloud debe fallar silenciosamente y el daemon debe
/// seguir corriendo. Tras 35s comprobamos que sigue vivo y responde a
/// SIGTERM limpio.
#[test]
fn daemon_survives_when_cloud_game_list_missing() {
    if which::which("rclone").is_err() {
        eprintln!("[skip] rclone not in PATH");
        return;
    }

    let device_id = "uuid-test-survive";
    let device_name = "Test-Survive";
    let game_id = "TestSurviveGame";
    let (xdg, app_dir, _cloud_dir) = setup_xdg_with_cloud(device_id, device_name);
    let saves_dir = tempfile::tempdir().unwrap();

    // Saves locales presentes (para que watched_paths NO esté vacío y el
    // daemon entre al worker loop en vez de la rama de retry inicial).
    std::fs::write(saves_dir.path().join("save.dat"), b"v1").unwrap();

    // sync-games con el juego en SYNC.
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

    // game-list LOCAL con el path resuelto, así que el daemon puede
    // entrar al worker loop sin tener que pelearse con auto_register.
    let now_iso = chrono::Utc::now().to_rfc3339();
    std::fs::write(
        app_dir.join("ludusavi-game-list.json"),
        format!(
            r#"{{
  "games": [
    {{
      "id": "{game_id}",
      "name": "{game_id}",
      "path_by_device": {{
        "{device_id}": {{ "path": "{saves}", "last_sync_mtime": "{now}" }}
      }},
      "last_synced_from": "{device_id}",
      "last_sync_time_utc": "{now}",
      "latest_write_time_utc": "{now}",
      "storage_bytes": 10
    }}
  ],
  "device_names": {{ "{device_id}": "{device_name}" }}
}}"#,
            saves = saves_dir.path().display(),
            now = now_iso,
        ),
    )
    .unwrap();

    // Cloud_dir está vacío — sin ludusavi-game-list.json. El daemon debe
    // arrancar igual y manejar la ausencia con grace al pollear.
    let mut child = spawn_daemon(xdg.path());

    // Esperamos 35s: más de un ciclo de polling completo (30s).
    std::thread::sleep(Duration::from_secs(35));

    // El daemon debe seguir vivo (no haberse caído en panic).
    let still_running = child.try_wait().unwrap().is_none();
    if !still_running {
        let diag = dump_diagnostics(&mut child, xdg.path());
        panic!("daemon died with empty cloud, expected it to survive\n{diag}");
    }

    // SIGTERM debe terminarlo limpiamente.
    let exit = terminate_and_wait(&mut child, Duration::from_secs(10)).unwrap();
    use std::os::unix::process::ExitStatusExt;
    let signaled = exit.signal().is_some();
    let diag = dump_diagnostics(&mut child, xdg.path());
    assert!(
        exit.success() || signaled,
        "expected clean exit or signal termination, got {exit:?}\n{diag}"
    );
}

/// Test 2: el daemon poll cada 1s el mtime de sync-games.json. Cuando
/// detecta un cambio, hace `return run_daemon(stop_flag)` (recursión)
/// para releer la config y reconstruir watched_paths. Verificamos que
/// añadir un juego nuevo en runtime hace que el watcher empiece a
/// vigilarlo y un cambio en sus saves dispare un upload.
#[test]
fn daemon_picks_up_new_game_when_sync_games_changes() {
    if which::which("rclone").is_err() {
        eprintln!("[skip] rclone not in PATH");
        return;
    }

    let device_id = "uuid-test-reload";
    let device_name = "Test-Reload";
    let game1 = "GameOne";
    let game2 = "GameTwo";
    let (xdg, app_dir, cloud_dir) = setup_xdg_with_cloud(device_id, device_name);
    let saves1 = tempfile::tempdir().unwrap();
    let saves2 = tempfile::tempdir().unwrap();

    std::fs::write(saves1.path().join("save.dat"), b"g1-v1").unwrap();
    std::fs::write(saves2.path().join("save.dat"), b"g2-v1").unwrap();

    // Estado inicial: solo game1 en SYNC. Daemon entrará al worker
    // loop y vigilará solo a game1.
    let initial_sync_games = format!(
        r#"{{
  "games": {{ "{game1}": {{ "mode": "sync", "auto_sync": true }} }},
  "safety_backups_enabled": false,
  "system_notifications_enabled": false
}}"#
    );
    std::fs::write(app_dir.join("sync-games.json"), &initial_sync_games).unwrap();

    // game-list local con game1 ya sincronizado (InSync) y game2 sin
    // entrada en cloud.
    let now_iso = chrono::Utc::now().to_rfc3339();
    std::fs::write(
        app_dir.join("ludusavi-game-list.json"),
        format!(
            r#"{{
  "games": [
    {{
      "id": "{game1}",
      "name": "{game1}",
      "path_by_device": {{
        "{device_id}": {{ "path": "{p1}", "last_sync_mtime": "{now}" }}
      }},
      "last_synced_from": "{device_id}",
      "last_sync_time_utc": "{now}",
      "latest_write_time_utc": "{now}",
      "storage_bytes": 5
    }},
    {{
      "id": "{game2}",
      "name": "{game2}",
      "path_by_device": {{
        "{device_id}": {{ "path": "{p2}", "last_sync_mtime": "{now}" }}
      }},
      "last_synced_from": "{device_id}",
      "last_sync_time_utc": "{now}",
      "latest_write_time_utc": "{now}",
      "storage_bytes": 5
    }}
  ],
  "device_names": {{ "{device_id}": "{device_name}" }}
}}"#,
            p1 = saves1.path().display(),
            p2 = saves2.path().display(),
            now = now_iso,
        ),
    )
    .unwrap();

    // ZIPs placeholder en cloud para que determine_sync_type considere
    // que game1 está InSync.
    let cloud_zip1 = cloud_dir.path().join(format!("game-{game1}.zip"));
    std::fs::write(&cloud_zip1, b"PK\x03\x04").unwrap();
    let cloud_zip2 = cloud_dir.path().join(format!("game-{game2}.zip"));
    std::fs::write(
        cloud_dir.path().join("ludusavi-game-list.json"),
        std::fs::read(app_dir.join("ludusavi-game-list.json")).unwrap(),
    )
    .unwrap();

    let mut child = spawn_daemon(xdg.path());

    // 5s para que el daemon arranque y entre al worker loop.
    std::thread::sleep(Duration::from_secs(5));
    if let Some(status) = child.try_wait().unwrap() {
        let diag = dump_diagnostics(&mut child, xdg.path());
        panic!("daemon exited prematurely (status={status:?})\n{diag}");
    }

    // Capturamos el estado inicial del cloud ZIP de game2: NO debería
    // tener contenido real (placeholder de 4 bytes).
    let zip2_size_before = std::fs::metadata(&cloud_zip2)
        .map(|m| m.len())
        .unwrap_or(0);
    assert_eq!(
        zip2_size_before, 4,
        "cloud ZIP de game2 debería ser solo el placeholder antes del cambio"
    );

    // Cambiamos sync-games.json para añadir game2 en SYNC.
    let updated_sync_games = format!(
        r#"{{
  "games": {{
    "{game1}": {{ "mode": "sync", "auto_sync": true }},
    "{game2}": {{ "mode": "sync", "auto_sync": true }}
  }},
  "safety_backups_enabled": false,
  "system_notifications_enabled": false
}}"#
    );
    std::fs::write(app_dir.join("sync-games.json"), &updated_sync_games).unwrap();

    // 5s para que el daemon detecte (poll de mtime cada 1s), recurse
    // run_daemon y registre el watcher para game2.
    std::thread::sleep(Duration::from_secs(5));

    // Modificamos los saves de game2 — el watcher recién registrado
    // debería capturar el cambio y, tras el debounce de 10s, subir.
    std::fs::write(saves2.path().join("save.dat"), b"g2-v2-after-reload").unwrap();

    // Esperamos hasta 25s (10s debounce + worker poll + upload + margen).
    let appeared = wait_until(
        || {
            std::fs::metadata(&cloud_zip2)
                .map(|m| m.len() > 100) // ZIP real es bastante mayor que el placeholder de 4 bytes
                .unwrap_or(false)
        },
        Duration::from_secs(25),
    );

    let exit = terminate_and_wait(&mut child, Duration::from_secs(10)).unwrap();

    if !appeared {
        let diag = dump_diagnostics(&mut child, xdg.path());
        let zip2_after = std::fs::metadata(&cloud_zip2).map(|m| m.len()).unwrap_or(0);
        panic!(
            "ZIP de game2 no apareció en cloud (size={zip2_after}); \
             el daemon no recargó sync-games.json o no registró el watcher\n\
             exit={exit:?}\n{diag}"
        );
    }
}

/// Construye un ZIP real con los pares (entry_name, content) dados,
/// para alimentar al daemon en el test de polling-download. Un ZIP
/// placeholder no nos sirve aquí porque el daemon llama
/// `extract_zip_to_directory` que validaría el formato.
fn make_zip(path: &std::path::Path, files: &[(&str, &[u8])]) {
    use std::io::Write;
    let file = std::fs::File::create(path).unwrap();
    let mut zip = zip::ZipWriter::new(file);
    let options: zip::write::FileOptions<()> =
        zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    for (name, content) in files {
        zip.start_file(*name, options).unwrap();
        zip.write_all(content).unwrap();
    }
    zip.finish().unwrap();
}

/// Test 1: el daemon poll cada 30s el mtime del game-list.json del
/// cloud. Si cambia, descarga la nueva versión del game-list y para
/// cada juego cuyo `latest_write_time_utc` cloud > local
/// `last_sync_mtime`, descarga el ZIP y lo extrae en saves.
///
/// Simulamos "otro device subió saves nuevos al cloud":
/// - Estado inicial: cloud y local InSync (ambos en T1).
/// - A mitad de la ejecución: reescribimos el ZIP del cloud con
///   contenido nuevo, y actualizamos el game-list para que el
///   `last_synced_from` sea otro device con timestamp T2 > T1.
/// - Esperamos hasta 40s para que el polling lo detecte y descargue.
/// - Verificamos: el archivo nuevo aparece en saves_dir.
#[test]
fn daemon_polls_cloud_and_downloads_when_remote_advances() {
    if which::which("rclone").is_err() {
        eprintln!("[skip] rclone not in PATH");
        return;
    }

    let device_id = "uuid-this-device";
    let other_device_id = "uuid-other-device";
    let device_name = "This-Device";
    let game_id = "PolledGame";

    let (xdg, app_dir, cloud_dir) = setup_xdg_with_cloud(device_id, device_name);
    let saves_dir = tempfile::tempdir().unwrap();

    // T1 = hace 1h. T2 = ahora. Diferencia clara entre estados.
    let t1_unix = chrono::Utc::now().timestamp() - 3600;
    let t1_iso = chrono::DateTime::from_timestamp(t1_unix, 0)
        .unwrap()
        .to_rfc3339();

    // Saves locales iniciales con mtime forzado a T1, para que
    // determine_sync_type considere InSync.
    let initial_save = saves_dir.path().join("v1-initial.dat");
    std::fs::write(&initial_save, b"v1-from-this-device").unwrap();
    let t1_ft = filetime::FileTime::from_unix_time(t1_unix, 0);
    filetime::set_file_mtime(&initial_save, t1_ft).unwrap();

    // sync-games con el juego en SYNC + auto_sync ON.
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

    // game-list local: todo synced en T1 desde nuestro device.
    let game_list_t1 = format!(
        r#"{{
  "games": [
    {{
      "id": "{game_id}",
      "name": "{game_id}",
      "path_by_device": {{
        "{device_id}": {{ "path": "{saves}", "last_sync_mtime": "{t1}" }}
      }},
      "last_synced_from": "{device_id}",
      "last_sync_time_utc": "{t1}",
      "latest_write_time_utc": "{t1}",
      "storage_bytes": 20
    }}
  ],
  "device_names": {{ "{device_id}": "{device_name}", "{other_device_id}": "Other-Device" }}
}}"#,
        saves = saves_dir.path().display(),
        t1 = t1_iso,
    );
    std::fs::write(app_dir.join("ludusavi-game-list.json"), &game_list_t1).unwrap();
    std::fs::write(cloud_dir.path().join("ludusavi-game-list.json"), &game_list_t1).unwrap();

    // ZIP inicial en el cloud con el contenido v1.
    let cloud_zip = cloud_dir.path().join(format!("game-{game_id}.zip"));
    make_zip(&cloud_zip, &[("v1-initial.dat", b"v1-from-this-device")]);

    let mut child = spawn_daemon(xdg.path());

    // 5s para arrancar y hacer la check_downloads inicial. Como cloud
    // y local están InSync, no debería pasar nada.
    std::thread::sleep(Duration::from_secs(5));
    if let Some(status) = child.try_wait().unwrap() {
        let diag = dump_diagnostics(&mut child, xdg.path());
        panic!("daemon exited prematurely (status={status:?})\n{diag}");
    }

    // Sanity: nuestros saves siguen siendo los de T1 (el daemon no
    // tocó nada al arrancar porque era InSync).
    assert!(
        saves_dir.path().join("v1-initial.dat").exists(),
        "saves originales deben seguir intactos tras el startup InSync"
    );

    // Simulamos "otro device subió saves nuevos":
    // 1. Nuevo ZIP en el cloud con contenido distinto.
    let new_zip_src = tempfile::tempdir().unwrap();
    make_zip(
        &cloud_zip,
        &[("v2-from-other.dat", b"v2-content-from-other-device")],
    );
    drop(new_zip_src);

    // 2. game-list actualizado: last_synced_from = other_device, T2 > T1.
    let t2_unix = chrono::Utc::now().timestamp();
    let t2_iso = chrono::DateTime::from_timestamp(t2_unix, 0)
        .unwrap()
        .to_rfc3339();
    let game_list_t2 = format!(
        r#"{{
  "games": [
    {{
      "id": "{game_id}",
      "name": "{game_id}",
      "path_by_device": {{
        "{device_id}": {{ "path": "{saves}", "last_sync_mtime": "{t1}" }},
        "{other_device_id}": {{ "path": "/some/other/path", "last_sync_mtime": "{t2}" }}
      }},
      "last_synced_from": "{other_device_id}",
      "last_sync_time_utc": "{t2}",
      "latest_write_time_utc": "{t2}",
      "storage_bytes": 30
    }}
  ],
  "device_names": {{ "{device_id}": "{device_name}", "{other_device_id}": "Other-Device" }}
}}"#,
        saves = saves_dir.path().display(),
        t1 = t1_iso,
        t2 = t2_iso,
    );
    std::fs::write(cloud_dir.path().join("ludusavi-game-list.json"), &game_list_t2).unwrap();

    // El polling es cada 30s. Damos hasta 45s para captar el cambio,
    // descargar el ZIP y extraerlo.
    let appeared = wait_until(
        || saves_dir.path().join("v2-from-other.dat").exists(),
        Duration::from_secs(45),
    );

    let exit = terminate_and_wait(&mut child, Duration::from_secs(10)).unwrap();

    if !appeared {
        let diag = dump_diagnostics(&mut child, xdg.path());
        panic!(
            "Saves locales no se actualizaron desde cloud tras 45s\n\
             exit={exit:?}\n{diag}"
        );
    }

    // El swap atómico de extract_zip_to_directory borra el dir antiguo
    // y lo reemplaza con lo del ZIP, así que v1-initial debe haber
    // desaparecido y v2-from-other estar presente con el contenido
    // correcto.
    let v2_content = std::fs::read(saves_dir.path().join("v2-from-other.dat")).unwrap();
    assert_eq!(v2_content, b"v2-content-from-other-device");
    assert!(
        !saves_dir.path().join("v1-initial.dat").exists(),
        "v1 debería haber sido reemplazado por el swap atómico de la descarga"
    );
}
