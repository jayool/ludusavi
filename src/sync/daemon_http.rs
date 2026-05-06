//! Servidor HTTP local del daemon — Capa 0 del plan Millennium/Decky.
//!
//! El daemon expone una API REST + un stream SSE en
//! `http://127.0.0.1:DAEMON_HTTP_PORT` para que los frontends (plugin
//! Millennium, plugin Decky, GUI Iced cuando se migre) hablen con el
//! daemon vía cliente-servidor en lugar del actual hack de "leer JSONs
//! cada 5s desde disco".
//!
//! Esta versión es minimal — sólo `GET /api/status` con auth via token.
//! Los demás endpoints (`/api/games`, `/api/devices`, `/api/events`)
//! llegarán en commits sucesivos.
//!
//! Convive con los JSONs existentes: el daemon escribe ambos (HTTP y
//! ficheros) hasta que la GUI Iced se migre. Migración gradual.

use std::sync::{atomic::{AtomicBool, Ordering}, Arc};
use std::time::Duration;

use axum::{
    extract::State,
    http::{header, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use tokio::net::TcpListener;
use tower_http::cors::{Any, CorsLayer};

use crate::prelude::{app_dir, StrictPath};

/// Puerto fijo en el que escucha el daemon. Elegido alto para no chocar
/// con servicios típicos. Documentado para que los plugins lo conozcan.
pub const DAEMON_HTTP_PORT: u16 = 61234;

/// Path del fichero del token, dentro de `app_dir()`. Los frontends
/// (plugin Millennium con backend Lua, plugin Decky, etc.) leen el
/// token de aquí en lugar de pedírselo al usuario o negociarlo via IPC.
fn token_path() -> StrictPath {
    app_dir().joined("daemon-token.txt")
}

/// Estado compartido del servidor. Por ahora sólo el token; en commits
/// sucesivos se ampliará con event broadcaster (para SSE) y con
/// referencias al estado del worker loop.
#[derive(Clone)]
struct AppState {
    token: Arc<String>,
}

/// Genera un token nuevo (40 hex chars, ~160 bits) usando sha1 sobre
/// time + pid + thread id + memory addresses.
///
/// No es crypto-random verdadero — para eso necesitaríamos `getrandom`
/// como dep directa. Para nuestro modelo de amenaza (otro proceso local
/// brute-forceando via HTTP) los 160 bits de "pseudo-random + secret
/// state-based" son suficientes: la única forma realista de saltarse el
/// auth es leer el fichero del token, que está en `~/...` con permisos
/// del usuario.
fn generate_token() -> String {
    use sha1::{Digest, Sha1};
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    use std::time::SystemTime;

    let mut sha = Sha1::new();

    // Inputs con varianza temporal y de proceso
    if let Ok(d) = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH) {
        sha.update(d.as_nanos().to_le_bytes());
    }
    sha.update(std::process::id().to_le_bytes());

    // Hash separado para mezclar el thread::id y el address de algunas
    // variables locales (entropía adicional sobre layout de memoria).
    let mut hasher = DefaultHasher::new();
    std::thread::current().id().hash(&mut hasher);
    let stack_var = 0u64;
    (&stack_var as *const _ as usize).hash(&mut hasher);
    sha.update(hasher.finish().to_le_bytes());

    // Otra ronda con sub-second nanos
    if let Ok(d) = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH) {
        sha.update(d.subsec_nanos().to_le_bytes());
    }

    let bytes = sha.finalize();
    bytes.iter().map(|b| format!("{:02x}", b)).collect::<String>()
}

/// Carga el token de `token_path()` o lo genera y persiste si no existe.
/// Devuelve el contenido para que el server lo valide en cada request.
///
/// Si el fichero existe pero está vacío o corrupto, regeneramos. No
/// queremos un token vacío sirviendo de bypass.
fn load_or_create_token() -> Result<String, String> {
    let path = token_path();
    if let Some(content) = path.read() {
        let trimmed = content.trim().to_string();
        if !trimmed.is_empty() {
            return Ok(trimmed);
        }
        log::warn!("[daemon-http] token file existed but was empty, regenerating");
    }

    let token = generate_token();
    path.create_parent_dir()
        .map_err(|e| format!("Failed to create app_dir for token: {e:?}"))?;
    path.write_with_content(&token)
        .map_err(|e| format!("Failed to persist daemon token: {e:?}"))?;
    log::info!("[daemon-http] generated new token at {}", path.render());
    Ok(token)
}

/// Middleware que valida el header `Authorization: Bearer <token>`
/// contra el token cargado al arrancar el server. 401 si falta o no
/// coincide.
async fn require_auth(
    State(state): State<AppState>,
    req: axum::extract::Request,
    next: Next,
) -> Response {
    let header_value = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    let expected = format!("Bearer {}", *state.token);
    if header_value != expected {
        log::debug!("[daemon-http] auth rejected for {} {}", req.method(), req.uri().path());
        return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({
                "error": "Missing or invalid Authorization header",
                "expected": "Bearer <daemon-token-from-disk>",
            })),
        )
            .into_response();
    }
    next.run(req).await
}

/// `GET /api/status` — endpoint mínimo para validar la plumbing entera.
/// Devuelve info estática del daemon. Más adelante incluirá juegos
/// activos, errores de rclone, último sync, etc.
async fn status_handler() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "daemon": "ludusavi-daemon",
        "version": env!("CARGO_PKG_VERSION"),
        "api_version": 1,
    }))
}

// ============================================================================
// /api/games — tabla denormalizada de juegos
// ============================================================================
//
// Combina las 3 fuentes que la GUI Iced lee por separado:
//   - ludusavi-game-list.json: metadatos de juegos (path por device,
//     timestamps, storage_bytes).
//   - daemon-status.json: status calculado por el worker loop (synced,
//     pending_backup, error, conflict, ...).
//   - sync-games.json: modo por juego (NONE/LOCAL/CLOUD/SYNC) +
//     auto_sync flag.
//
// El plugin recibe ya el join — no tiene que leer 3 ficheros ni saber
// cómo cruzarlos. Mantiene la lógica complicada en el daemon, donde ya
// vive.

/// Detalle de un error transitorio reportado por el daemon (rclone caído,
/// permisos, etc.). Solo presente cuando `status == "error"`.
#[derive(serde::Serialize)]
struct ApiErrorInfo {
    category: String,
    direction: String,
    message: String,
}

/// Detalle de un conflicto bidireccional. Solo presente cuando
/// `status == "conflict"`. Permite al plugin renderizar el banner de
/// resolución (Keep local / Keep cloud / Keep both) con los timestamps
/// correctos.
#[derive(serde::Serialize)]
struct ApiConflictInfo {
    local_time: Option<String>,
    cloud_time: Option<String>,
    /// Nombre legible del device (no UUID) que subió la versión cloud.
    cloud_from: Option<String>,
}

#[derive(serde::Serialize)]
struct ApiGameRow {
    name: String,
    /// "none" | "local" | "cloud" | "sync".
    mode: String,
    auto_sync: bool,
    /// "synced" | "pending_backup" | "pending_restore" | "not_managed"
    /// | "error" | "conflict". Sólo presente si el daemon ha calculado
    /// status para este juego (puede faltar para juegos NONE en LOCAL
    /// sin entrada en game-list).
    status: String,
    /// True si tiene path registrado en este device en game-list.
    registered_here: bool,
    /// True si está registrado en algún device DISTINTO del actual.
    registered_elsewhere: bool,
    /// Path local de los saves, si está registrado en este device.
    save_path: Option<String>,
    /// Nombre legible del device que hizo el último sync (no UUID).
    last_synced_from: Option<String>,
    last_sync_time_utc: Option<String>,
    latest_write_time_utc: Option<String>,
    storage_bytes: u64,
    error: Option<ApiErrorInfo>,
    conflict: Option<ApiConflictInfo>,
}

#[derive(serde::Serialize)]
struct ApiDevice {
    id: String,
    name: String,
}

#[derive(serde::Serialize)]
struct ApiGamesResponse {
    device: ApiDevice,
    games: Vec<ApiGameRow>,
    /// UUID → nombre legible. Útil para plugins que quieran resolver
    /// uuids de last_synced_from de otros juegos (p.ej. la pantalla
    /// "All devices" del draft).
    device_names: std::collections::HashMap<String, String>,
    /// Si rclone está caído, los plugins pueden mostrar un banner
    /// global. El daemon escribe este flag en daemon-status.json.
    rclone_missing: bool,
}

/// Lee el daemon-status.json crudo (formato escrito por
/// `write_sync_status_with_errors`). Devuelve un map vacío si el
/// fichero no existe o está corrupto.
fn read_status_json(app_dir: &StrictPath) -> serde_json::Value {
    let path = app_dir.joined("daemon-status.json");
    path.read()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .unwrap_or(serde_json::Value::Null)
}

fn read_game_list(app_dir: &StrictPath) -> crate::sync::game_list::GameListFile {
    let path = app_dir.joined("ludusavi-game-list.json");
    path.read()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn read_sync_config(app_dir: &StrictPath) -> crate::sync::sync_config::SyncGamesConfig {
    let path = app_dir.joined("sync-games.json");
    path.read()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// Construye la respuesta de `/api/games` combinando game-list +
/// daemon-status + sync-games + device. `app_dir` se inyecta como
/// parámetro para que los tests puedan apuntar a un tempdir; el handler
/// real lo pasa desde `app_dir()`.
fn build_games_response(app_dir: &StrictPath) -> ApiGamesResponse {
    use crate::sync::sync_config::SaveMode;

    let device = crate::sync::device::DeviceIdentity::load_or_create(app_dir);
    let game_list = read_game_list(app_dir);
    let sync_config = read_sync_config(app_dir);
    let status_root = read_status_json(app_dir);
    let status_games = status_root.get("games").cloned().unwrap_or_default();
    let rclone_missing = status_root
        .get("rclone_missing")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    fn mode_str(m: &SaveMode) -> &'static str {
        match m {
            SaveMode::None => "none",
            SaveMode::Local => "local",
            SaveMode::Cloud => "cloud",
            SaveMode::Sync => "sync",
        }
    }

    // Construir la unión de game IDs: game-list ∪ sync-games. El plugin
    // ve TODOS los juegos que el daemon conoce, no sólo los del cloud.
    let mut all_names: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for g in &game_list.games {
        all_names.insert(g.id.clone());
    }
    for g in sync_config.games.keys() {
        all_names.insert(g.clone());
    }

    let mut rows: Vec<ApiGameRow> = Vec::with_capacity(all_names.len());
    for name in all_names {
        let game = game_list.get_game(&name);
        let mode = sync_config.get_mode(&name);
        let auto_sync = sync_config.get_auto_sync(&name);

        let status_obj = status_games.get(&name);
        let status = status_obj
            .and_then(|v| v.get("status"))
            .and_then(|v| v.as_str())
            .unwrap_or(if matches!(mode, SaveMode::None) {
                "not_managed"
            } else {
                ""
            })
            .to_string();

        let registered_here = game
            .map(|g| g.path_by_device.contains_key(&device.id))
            .unwrap_or(false);
        let registered_elsewhere = game
            .map(|g| {
                g.path_by_device
                    .keys()
                    .any(|dev| dev != &device.id)
            })
            .unwrap_or(false);

        let save_path = game.and_then(|g| g.get_path(&device.id).map(String::from));

        let last_synced_from = game
            .and_then(|g| g.last_synced_from.as_ref())
            .map(|uuid| game_list.get_device_name(uuid).to_string());
        let last_sync_time_utc = game.and_then(|g| g.last_sync_time_utc.map(|t| t.to_rfc3339()));
        let latest_write_time_utc = game.and_then(|g| g.latest_write_time_utc.map(|t| t.to_rfc3339()));
        let storage_bytes = game.map(|g| g.storage_bytes).unwrap_or(0);

        let error = if status == "error" {
            status_obj.map(|s| ApiErrorInfo {
                category: s.get("error_category").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                direction: s.get("error_direction").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                message: s.get("error_message").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            })
        } else {
            None
        };

        let conflict = if status == "conflict" {
            status_obj.map(|s| ApiConflictInfo {
                local_time: s.get("conflict_local_time").and_then(|v| v.as_str()).map(String::from),
                cloud_time: s.get("conflict_cloud_time").and_then(|v| v.as_str()).map(String::from),
                cloud_from: s
                    .get("conflict_cloud_from")
                    .and_then(|v| v.as_str())
                    .map(|uuid| game_list.get_device_name(uuid).to_string()),
            })
        } else {
            None
        };

        rows.push(ApiGameRow {
            name,
            mode: mode_str(&mode).to_string(),
            auto_sync,
            status,
            registered_here,
            registered_elsewhere,
            save_path,
            last_synced_from,
            last_sync_time_utc,
            latest_write_time_utc,
            storage_bytes,
            error,
            conflict,
        });
    }

    ApiGamesResponse {
        device: ApiDevice {
            id: device.id.clone(),
            name: device.name.clone(),
        },
        games: rows,
        device_names: game_list.device_names.clone(),
        rclone_missing,
    }
}

async fn games_handler() -> Json<ApiGamesResponse> {
    Json(build_games_response(&app_dir()))
}

// ============================================================================
// /api/devices — lista de dispositivos del game-list
// ============================================================================
//
// Equivalente a la pantalla "All Devices" de la GUI Iced. Para cada
// dispositivo que aparece en `path_by_device` de algún juego, devuelve
// su id + nombre + lista de juegos registrados + último sync agregado.
// El plugin filtra/ordena/agrupa según la UI que esté renderizando
// (panel principal, lateral del Decky, etc.).

#[derive(serde::Serialize)]
struct ApiDeviceRow {
    id: String,
    name: String,
    /// True si es el device en el que está corriendo este daemon.
    is_current: bool,
    /// Names de juegos que tienen path registrado en este device
    /// (es decir, donde `path_by_device.contains(device_id)`).
    games: Vec<String>,
    /// `max(last_sync_time_utc)` entre los juegos de este device, o
    /// `None` si ninguno tiene timestamp.
    last_sync_time_utc: Option<String>,
}

#[derive(serde::Serialize)]
struct ApiDevicesResponse {
    current_device_id: String,
    devices: Vec<ApiDeviceRow>,
}

fn build_devices_response(app_dir: &StrictPath) -> ApiDevicesResponse {
    let device = crate::sync::device::DeviceIdentity::load_or_create(app_dir);
    let game_list = read_game_list(app_dir);

    // Agrupar: device_id -> (Vec<game_name>, max last_sync_time)
    type DeviceAggregate = (Vec<String>, Option<chrono::DateTime<chrono::Utc>>);
    let mut by_device: std::collections::HashMap<String, DeviceAggregate> =
        std::collections::HashMap::new();

    for game in &game_list.games {
        for dev_id in game.path_by_device.keys() {
            let entry = by_device
                .entry(dev_id.clone())
                .or_insert_with(|| (Vec::new(), None));
            entry.0.push(game.id.clone());
            // Acumular el max timestamp.
            if let Some(t) = game.last_sync_time_utc {
                entry.1 = Some(entry.1.map(|cur| cur.max(t)).unwrap_or(t));
            }
        }
    }

    let mut devices: Vec<ApiDeviceRow> = by_device
        .into_iter()
        .map(|(id, (mut games, last_sync))| {
            games.sort();
            ApiDeviceRow {
                name: game_list.get_device_name(&id).to_string(),
                is_current: id == device.id,
                last_sync_time_utc: last_sync.map(|t| t.to_rfc3339()),
                games,
                id,
            }
        })
        .collect();

    // Orden estable: current primero, luego por nombre alfabéticamente.
    devices.sort_by(|a, b| match (a.is_current, b.is_current) {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
    });

    ApiDevicesResponse {
        current_device_id: device.id,
        devices,
    }
}

async fn devices_handler() -> Json<ApiDevicesResponse> {
    Json(build_devices_response(&app_dir()))
}

/// Construye el `Router` de axum con todos los endpoints + auth + CORS.
fn build_router(state: AppState) -> Router {
    // CORS abierto a localhost en cualquier puerto. Los plugins de
    // Millennium/Decky corren bajo orígenes que no controlamos
    // (`steam://...`, `chrome-extension://...`), así que `Any` es lo
    // razonable. La auth via token es lo que protege la API, no el CORS.
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE]);

    Router::new()
        .route("/api/status", get(status_handler))
        .route("/api/games", get(games_handler))
        .route("/api/devices", get(devices_handler))
        // El middleware de auth se aplica DESPUÉS del cors layer para
        // que las peticiones OPTIONS (preflight) no requieran token.
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            require_auth,
        ))
        .with_state(state)
        .layer(cors)
}

/// Arranca el servidor HTTP y bloquea hasta que `stop_flag` se active.
/// Llamado desde el binario del daemon en paralelo al worker loop.
pub fn run_http_server(stop_flag: Arc<AtomicBool>) -> Result<(), String> {
    let token = load_or_create_token()?;
    let state = AppState {
        token: Arc::new(token),
    };

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("daemon-http")
        .build()
        .map_err(|e| format!("Failed to create tokio runtime: {e}"))?;

    runtime.block_on(async move {
        let app = build_router(state);
        let addr = format!("127.0.0.1:{}", DAEMON_HTTP_PORT);
        let listener = TcpListener::bind(&addr)
            .await
            .map_err(|e| format!("Failed to bind {addr}: {e}"))?;
        log::info!("[daemon-http] listening on http://{addr}");

        let stop = stop_flag.clone();
        axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                while !stop.load(Ordering::Relaxed) {
                    tokio::time::sleep(Duration::from_millis(500)).await;
                }
                log::info!("[daemon-http] stop signal received, shutting down");
            })
            .await
            .map_err(|e| format!("HTTP server error: {e}"))?;
        Ok::<_, String>(())
    })?;

    log::info!("[daemon-http] stopped");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt; // for `.oneshot()`

    #[test]
    fn generated_tokens_are_40_hex_chars() {
        let token = generate_token();
        assert_eq!(token.len(), 40, "expected sha1 hex (40 chars), got {}", token.len());
        assert!(token.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn consecutive_tokens_differ() {
        // No es crypto-random pero debería variar entre llamadas porque
        // mezcla time::now() (nanos) y direcciones de memoria.
        let a = generate_token();
        let b = generate_token();
        assert_ne!(a, b, "tokens should differ between calls");
    }

    fn test_state(token: &str) -> AppState {
        AppState {
            token: Arc::new(token.to_string()),
        }
    }

    #[tokio::test]
    async fn status_returns_401_without_auth_header() {
        let app = build_router(test_state("test-token"));
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/status")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn status_returns_401_with_wrong_token() {
        let app = build_router(test_state("right-token"));
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/status")
                    .header("Authorization", "Bearer wrong-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn status_returns_200_with_correct_token() {
        let token = "correct-token-abc";
        let app = build_router(test_state(token));
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/status")
                    .header("Authorization", format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        // Body parses as our JSON shape.
        let bytes = axum::body::to_bytes(response.into_body(), 1024).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["daemon"], "ludusavi-daemon");
        assert_eq!(json["api_version"], 1);
    }

    /// Helper de tests: escribe los 4 ficheros que `build_games_response`
    /// lee del app_dir (game-list, sync-games, daemon-status, device).
    fn seed_app_dir(
        app_dir: &std::path::Path,
        device_id: &str,
        device_name: &str,
        game_list_json: &str,
        sync_games_json: &str,
        daemon_status_json: Option<&str>,
    ) {
        std::fs::create_dir_all(app_dir).unwrap();
        std::fs::write(
            app_dir.join("ludusavi-device.json"),
            format!(r#"{{"id":"{device_id}","name":"{device_name}"}}"#),
        )
        .unwrap();
        std::fs::write(app_dir.join("ludusavi-game-list.json"), game_list_json).unwrap();
        std::fs::write(app_dir.join("sync-games.json"), sync_games_json).unwrap();
        if let Some(status) = daemon_status_json {
            std::fs::write(app_dir.join("daemon-status.json"), status).unwrap();
        }
    }

    fn sp(p: &std::path::Path) -> StrictPath {
        StrictPath::new(p.to_string_lossy().to_string())
    }

    #[test]
    fn games_response_empty_when_no_files() {
        let tmp = tempfile::tempdir().unwrap();
        // Sin escribir nada: el daemon nunca ha corrido en este app_dir.
        let resp = build_games_response(&sp(tmp.path()));
        assert_eq!(resp.games.len(), 0);
        // El device se autogenera en load_or_create — siempre tiene id válido.
        assert!(!resp.device.id.is_empty());
        assert_eq!(resp.rclone_missing, false);
    }

    #[test]
    fn games_response_combines_three_sources() {
        let tmp = tempfile::tempdir().unwrap();
        let device_id = "uuid-test-aaa";
        let device_name = "Test-PC";
        let other_id = "uuid-other-device";

        let game_list = format!(
            r#"{{
              "games": [
                {{
                  "id": "Hades",
                  "name": "Hades",
                  "path_by_device": {{
                    "{device_id}": {{ "path": "C:/saves/hades", "last_sync_mtime": "2026-05-06T12:00:00Z" }}
                  }},
                  "last_synced_from": "{device_id}",
                  "last_sync_time_utc": "2026-05-06T12:00:00Z",
                  "latest_write_time_utc": "2026-05-06T12:00:00Z",
                  "storage_bytes": 1024
                }},
                {{
                  "id": "Skyrim",
                  "name": "Skyrim",
                  "path_by_device": {{
                    "{other_id}": {{ "path": "/home/jayo/skyrim", "last_sync_mtime": "2026-05-05T10:00:00Z" }}
                  }},
                  "last_synced_from": "{other_id}",
                  "last_sync_time_utc": "2026-05-05T10:00:00Z",
                  "latest_write_time_utc": "2026-05-05T10:00:00Z",
                  "storage_bytes": 2048
                }}
              ],
              "device_names": {{ "{device_id}": "{device_name}", "{other_id}": "Steam-Deck" }}
            }}"#,
        );
        let sync_games = r#"{
          "games": {
            "Hades": { "mode": "sync", "auto_sync": true },
            "Skyrim": { "mode": "none", "auto_sync": false }
          },
          "safety_backups_enabled": true,
          "system_notifications_enabled": true
        }"#;
        let daemon_status = r#"{
          "games": {
            "Hades": {
              "status": "synced",
              "last_sync_time": "2026-05-06T12:00:00Z",
              "last_local_write": "2026-05-06T12:00:00Z",
              "error_category": null,
              "error_direction": null,
              "error_message": null,
              "conflict_local_time": null,
              "conflict_cloud_time": null,
              "conflict_cloud_from": null
            }
          },
          "rclone_missing": false
        }"#;

        seed_app_dir(
            tmp.path(),
            device_id,
            device_name,
            &game_list,
            sync_games,
            Some(daemon_status),
        );

        let resp = build_games_response(&sp(tmp.path()));

        assert_eq!(resp.device.id, device_id);
        assert_eq!(resp.device.name, device_name);
        assert_eq!(resp.games.len(), 2);

        let hades = resp.games.iter().find(|g| g.name == "Hades").unwrap();
        assert_eq!(hades.mode, "sync");
        assert!(hades.auto_sync);
        assert_eq!(hades.status, "synced");
        assert!(hades.registered_here, "Hades has path for our device");
        assert!(!hades.registered_elsewhere);
        assert_eq!(hades.save_path.as_deref(), Some("C:/saves/hades"));
        // last_synced_from se resuelve a nombre legible (no UUID).
        assert_eq!(hades.last_synced_from.as_deref(), Some(device_name));
        assert_eq!(hades.storage_bytes, 1024);
        assert!(hades.error.is_none());
        assert!(hades.conflict.is_none());

        let skyrim = resp.games.iter().find(|g| g.name == "Skyrim").unwrap();
        assert_eq!(skyrim.mode, "none");
        // Sin status calculado por daemon, el handler infiere "not_managed"
        // porque el modo es None.
        assert_eq!(skyrim.status, "not_managed");
        assert!(!skyrim.registered_here);
        assert!(skyrim.registered_elsewhere, "Skyrim has path for other device");
        assert!(skyrim.save_path.is_none(), "no save_path for our device");
        // last_synced_from también se resuelve a nombre legible.
        assert_eq!(skyrim.last_synced_from.as_deref(), Some("Steam-Deck"));
    }

    #[test]
    fn games_response_surfaces_error_details() {
        let tmp = tempfile::tempdir().unwrap();
        let device_id = "uuid-aaa";

        let game_list = format!(
            r#"{{
              "games": [{{
                "id": "BrokenGame",
                "name": "BrokenGame",
                "path_by_device": {{
                  "{device_id}": {{ "path": "C:/saves/x" }}
                }},
                "last_synced_from": null,
                "last_sync_time_utc": null,
                "latest_write_time_utc": null,
                "storage_bytes": 0
              }}],
              "device_names": {{ "{device_id}": "Test" }}
            }}"#,
        );
        let sync_games = r#"{
          "games": { "BrokenGame": { "mode": "sync", "auto_sync": true } },
          "safety_backups_enabled": true,
          "system_notifications_enabled": true
        }"#;
        let daemon_status = r#"{
          "games": {
            "BrokenGame": {
              "status": "error",
              "last_sync_time": "",
              "last_local_write": "",
              "error_category": "rclone",
              "error_direction": "upload",
              "error_message": "rclone token expired",
              "conflict_local_time": null,
              "conflict_cloud_time": null,
              "conflict_cloud_from": null
            }
          },
          "rclone_missing": true
        }"#;

        seed_app_dir(
            tmp.path(),
            device_id,
            "Test",
            &game_list,
            sync_games,
            Some(daemon_status),
        );

        let resp = build_games_response(&sp(tmp.path()));
        assert!(resp.rclone_missing, "rclone_missing flag propagado");
        let game = &resp.games[0];
        assert_eq!(game.status, "error");
        let err = game.error.as_ref().expect("error info present when status=error");
        assert_eq!(err.category, "rclone");
        assert_eq!(err.direction, "upload");
        assert_eq!(err.message, "rclone token expired");
    }

    #[test]
    fn devices_response_empty_when_no_files() {
        let tmp = tempfile::tempdir().unwrap();
        let resp = build_devices_response(&sp(tmp.path()));
        assert_eq!(resp.devices.len(), 0);
        assert!(!resp.current_device_id.is_empty());
    }

    #[test]
    fn devices_response_aggregates_games_per_device_and_sorts_current_first() {
        let tmp = tempfile::tempdir().unwrap();
        let me = "uuid-me";
        let other = "uuid-other";
        let zzz = "uuid-zzz-no-name";

        let game_list = format!(
            r#"{{
              "games": [
                {{
                  "id": "Hades",
                  "name": "Hades",
                  "path_by_device": {{
                    "{me}":   {{ "path": "C:/saves/hades" }},
                    "{other}":{{ "path": "/home/saves/hades" }}
                  }},
                  "last_synced_from": "{me}",
                  "last_sync_time_utc": "2026-05-06T12:00:00Z",
                  "latest_write_time_utc": "2026-05-06T12:00:00Z",
                  "storage_bytes": 100
                }},
                {{
                  "id": "Skyrim",
                  "name": "Skyrim",
                  "path_by_device": {{
                    "{other}":{{ "path": "/home/saves/skyrim" }},
                    "{zzz}":  {{ "path": "/home/saves/skyrim" }}
                  }},
                  "last_synced_from": "{other}",
                  "last_sync_time_utc": "2026-05-07T08:00:00Z",
                  "latest_write_time_utc": "2026-05-07T08:00:00Z",
                  "storage_bytes": 200
                }}
              ],
              "device_names": {{ "{me}": "My-PC", "{other}": "Steam-Deck" }}
            }}"#,
        );
        // sync-games no influye en /api/devices, basta con un objeto vacío.
        let sync_games = r#"{ "games": {}, "safety_backups_enabled": true, "system_notifications_enabled": true }"#;

        seed_app_dir(tmp.path(), me, "My-PC", &game_list, sync_games, None);

        let resp = build_devices_response(&sp(tmp.path()));

        assert_eq!(resp.current_device_id, me);
        assert_eq!(resp.devices.len(), 3, "3 devices distintos en path_by_device");

        // Current primero. Resto orden alfabético por nombre legible:
        // My-PC (current), Steam-Deck, uuid-zzz-no-name (sin nombre legible).
        assert!(resp.devices[0].is_current);
        assert_eq!(resp.devices[0].id, me);
        assert_eq!(resp.devices[0].games, vec!["Hades".to_string()]);

        assert_eq!(resp.devices[1].name, "Steam-Deck");
        assert_eq!(resp.devices[1].games, vec!["Hades".to_string(), "Skyrim".to_string()]);

        // Device sin entry en device_names: el nombre cae al UUID por defecto.
        assert_eq!(resp.devices[2].name, zzz);
        assert_eq!(resp.devices[2].games, vec!["Skyrim".to_string()]);

        // last_sync_time_utc agregado: max de los timestamps de sus games.
        // Steam-Deck tiene Hades(2026-05-06) y Skyrim(2026-05-07) → 2026-05-07.
        assert_eq!(
            resp.devices[1].last_sync_time_utc.as_deref(),
            Some("2026-05-07T08:00:00+00:00")
        );
    }

    #[tokio::test]
    async fn cors_preflight_does_not_require_auth() {
        // Las peticiones OPTIONS preflight deben responder sin token —
        // el navegador del plugin las hace antes del request real para
        // descubrir qué headers acepta el server. Si requirieran auth,
        // ningún plugin podría hacer fetch desde un origin distinto.
        let app = build_router(test_state("any"));
        let response = app
            .oneshot(
                Request::builder()
                    .method("OPTIONS")
                    .uri("/api/status")
                    .header("Origin", "https://example.com")
                    .header("Access-Control-Request-Method", "GET")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        // 200 OK o 204 No Content son ambos válidos para preflight.
        assert!(
            response.status().is_success(),
            "expected 2xx, got {}",
            response.status()
        );
    }
}
