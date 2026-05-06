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
    routing::{get, post},
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

/// Eventos que el daemon empuja a los frontends conectados via SSE.
/// El plugin reacciona re-fetcheando el endpoint correspondiente.
///
/// Granularidad coarse-grained a propósito: el daemon detecta cambios
/// observando los JSONs que escribe el worker loop (option B de Fase 0),
/// así que sabe "algo cambió en games" pero no "qué juego concreto".
/// El plugin re-fetchea `/api/games` entero — barato, una request HTTP
/// local, sin coste de red real.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "type")]
pub enum DaemonEvent {
    /// El estado de los juegos cambió (status, sync time, errores, o el
    /// game-list añadió/quitó juegos). Plugin re-fetchea `/api/games`.
    #[serde(rename = "games_changed")]
    GamesChanged,
    /// La lista de devices cambió (nuevo device sincronizó, rename de
    /// device, etc.). Plugin re-fetchea `/api/devices`.
    #[serde(rename = "devices_changed")]
    DevicesChanged,
    /// El daemon reinició su worker loop (cambio en sync-games.json).
    /// Plugin re-fetchea todo para evitar drift.
    #[serde(rename = "daemon_restarted")]
    DaemonRestarted,
}

/// Capacidad del canal de broadcast. Si se llenan, los receivers más
/// lentos pierden eventos antiguos (dropping the oldest). 64 da margen
/// de sobra para ráfagas — un usuario con 50 juegos no genera más de
/// unos pocos eventos por minuto.
const EVENT_CHANNEL_CAPACITY: usize = 64;

/// Estado compartido del servidor: el token (auth) y el broadcaster
/// de eventos (SSE). `events.subscribe()` crea un receiver nuevo por
/// cliente conectado; `events.send(...)` empuja a todos los suscriptores.
#[derive(Clone)]
struct AppState {
    token: Arc<String>,
    events: tokio::sync::broadcast::Sender<DaemonEvent>,
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

/// Middleware que valida el token via DOS mecanismos:
///
/// 1. `Authorization: Bearer <token>` header (preferido, lo usan los
///    `fetch` normales).
/// 2. `?token=<token>` query string (sólo necesario para `EventSource`
///    en el plugin, porque la API estándar de `EventSource` NO permite
///    headers custom).
///
/// Ambos canales devuelven 401 si falta o no coincide.
///
/// Nota de seguridad: el daemon escucha en localhost únicamente, por lo
/// que el query token no puede leakear a la red. Aún así, los headers
/// son preferibles porque NO se loggean en `Referer`, history del
/// navegador, ni proxies. Por convención el plugin usa header en
/// `fetch` y query sólo en `EventSource`.
async fn require_auth(
    State(state): State<AppState>,
    req: axum::extract::Request,
    next: Next,
) -> Response {
    let expected = state.token.as_str();

    // Canal 1: Authorization header.
    let header_token = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "));

    // Canal 2: ?token=... en el query string. Parseo manual para no
    // tirar de `axum::extract::Query` (querría una struct específica
    // y nos romperia si añadimos otros params al endpoint).
    let query_token = req.uri().query().and_then(|q| {
        q.split('&').find_map(|kv| {
            let mut parts = kv.splitn(2, '=');
            match (parts.next(), parts.next()) {
                (Some("token"), Some(v)) => Some(v),
                _ => None,
            }
        })
    });

    let provided = header_token.or(query_token).unwrap_or("");
    if provided != expected {
        log::debug!(
            "[daemon-http] auth rejected for {} {}",
            req.method(),
            req.uri().path()
        );
        return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({
                "error": "Missing or invalid token",
                "expected": "Authorization: Bearer <daemon-token-from-disk>, or ?token=<token> on /api/events",
            })),
        )
            .into_response();
    }
    next.run(req).await
}

/// `GET /api/status` — info de salud del daemon.
///
/// Además del check básico de plumbing, devuelve:
///  - `app_dir`: path al directorio de Ludusavi en el SO (los plugins
///    no pueden leer disco directamente por el sandbox CEF, pero sí
///    pasarle este path a una callable de su backend Lua/Python para
///    abrirlo con el explorador).
///  - `last_sync_time_utc`: último mod_time global registrado por el
///    daemon (de `daemon-state.json`/`last_known_mod_time`). Es el
///    "Last sync" que la GUI muestra en el card SYNC DAEMON.
///  - `running`: aquí siempre `true` — si el cliente recibe la
///    respuesta es que el daemon está vivo. El campo existe para que
///    el plugin pueda renderizar el dot verde "Daemon is running"
///    sin condicionales especiales.
async fn status_handler() -> Json<serde_json::Value> {
    let app_dir_path = app_dir();
    let last_sync = read_last_sync_time(&app_dir_path);

    Json(serde_json::json!({
        "daemon": "ludusavi-daemon",
        "version": env!("CARGO_PKG_VERSION"),
        "api_version": 1,
        "app_dir": app_dir_path.render(),
        "last_sync_time_utc": last_sync,
        "running": true,
    }))
}

/// Lee `daemon-state.json` y extrae el campo `last_known_mod_time`,
/// que el worker actualiza cada vez que detecta un cambio de mod-time
/// en el game-list. Es el "Last sync" del daemon a nivel global —
/// distinto del per-device `last_sync_time_utc`. Devuelve None si el
/// fichero no existe o no contiene el campo.
fn read_last_sync_time(app_dir: &StrictPath) -> Option<String> {
    let path = app_dir.joined("daemon-state.json");
    let content = path.read()?;
    let json: serde_json::Value = serde_json::from_str(&content).ok()?;
    json.get("last_known_mod_time")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

// ============================================================================
// /api/cloud — config de cloud storage (Provider, Remote ID, Path, rclone)
// ============================================================================
//
// Equivale a la "CLOUD STORAGE card" de Screen::ThisDevice en la GUI.
// El plugin necesita esto para pintar la misma card. Read-only — el
// usuario configura cloud desde la GUI Iced (o, en el futuro, via POST
// endpoint en Fase 2).

#[derive(serde::Serialize)]
struct ApiCloudResponse {
    /// "Google Drive" / "Dropbox" / "OneDrive" / "Box" / "FTP" / "SMB"
    /// / "WebDAV" / "Custom" / "Not configured".
    provider: String,
    /// rclone remote ID. "—" si no hay remote configurado.
    remote_id: String,
    /// Carpeta cloud para los backups (p.ej. "ludusavi-backup").
    path: String,
    /// Estado del binario rclone: "ok" (config válida y binario
    /// encontrado), "not_configured" (path en config vacío o inválido),
    /// "missing" (el daemon worker reportó que el binario no existe
    /// en runtime — por flag en daemon-status.json).
    rclone_state: String,
    /// URL para instalar rclone si está missing — el plugin la usa
    /// para el botón "Install instructions" como hace la GUI.
    install_url: String,
    /// Path al binario rclone (lo que tiene `config.apps.rclone.path`).
    /// Necesario para la card CLOUD/RCLONE de la pantalla Settings.
    rclone_executable: String,
    /// Flags globales que el daemon pasa a rclone (p.ej.
    /// "--fast-list --ignore-checksum"). Necesario para la misma card.
    rclone_arguments: String,
}

async fn cloud_handler() -> Json<ApiCloudResponse> {
    use crate::resource::config::Config;

    let config = match Config::load() {
        Ok(c) => c,
        Err(e) => {
            log::warn!("[daemon-http] config load failed for cloud: {e:?}");
            return Json(ApiCloudResponse {
                provider: "Not configured".to_string(),
                remote_id: "—".to_string(),
                path: String::new(),
                rclone_state: "not_configured".to_string(),
                install_url: "https://rclone.org/downloads/".to_string(),
                rclone_executable: String::new(),
                rclone_arguments: String::new(),
            });
        }
    };

    let provider = match &config.cloud.remote {
        Some(crate::cloud::Remote::GoogleDrive { .. }) => "Google Drive",
        Some(crate::cloud::Remote::Dropbox { .. }) => "Dropbox",
        Some(crate::cloud::Remote::OneDrive { .. }) => "OneDrive",
        Some(crate::cloud::Remote::Box { .. }) => "Box",
        Some(crate::cloud::Remote::Ftp { .. }) => "FTP",
        Some(crate::cloud::Remote::Smb { .. }) => "SMB",
        Some(crate::cloud::Remote::WebDav { .. }) => "WebDAV",
        Some(crate::cloud::Remote::Custom { .. }) => "Custom",
        None => "Not configured",
    }
    .to_string();

    let remote_id = config
        .cloud
        .remote
        .as_ref()
        .map(|r| r.id().to_string())
        .unwrap_or_else(|| "—".to_string());

    // 3-way: missing (binario no existe runtime) > ok (config válida)
    // > not_configured (path vacío o inválido). Mismo orden que la GUI.
    let rclone_missing_flag = read_rclone_missing_flag(&app_dir());
    let rclone_state = if rclone_missing_flag {
        "missing"
    } else if config.apps.rclone.is_valid() {
        "ok"
    } else {
        "not_configured"
    }
    .to_string();

    Json(ApiCloudResponse {
        provider,
        remote_id,
        path: config.cloud.path.clone(),
        rclone_state,
        install_url: "https://rclone.org/downloads/".to_string(),
        rclone_executable: config.apps.rclone.path.raw().to_string(),
        rclone_arguments: config.apps.rclone.arguments.clone(),
    })
}

/// Lee el flag `rclone_missing` de `daemon-status.json`. El worker lo
/// escribe vía `write_rclone_missing_flag` cuando descubre que el
/// binario rclone no existe en runtime.
fn read_rclone_missing_flag(app_dir: &StrictPath) -> bool {
    let path = app_dir.joined("daemon-status.json");
    let Some(content) = path.read() else {
        return false;
    };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&content) else {
        return false;
    };
    json.get("rclone_missing")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

// ============================================================================
// /api/settings — config completa relevante para la pantalla Settings
// ============================================================================
//
// Equivale a Screen::Other ("Settings") de la GUI Iced. Devuelve cada
// card como una sub-estructura: backup_path, manifest, roots, safety,
// service. La info de cloud sigue en /api/cloud (que ya añadimos antes
// para la card CLOUD STORAGE de This Device) — el plugin se trae las
// dos al cargar la pantalla Settings.
//
// Read-only en Fase 1. Los toggles (safety, system_notifications) y
// los botones (Install service, Refresh manifest, etc.) se enchufan
// con POST endpoints en Fase 2.

#[derive(serde::Serialize)]
struct ApiSettingsResponse {
    backup_path: String,
    manifest: ApiSettingsManifest,
    roots: Vec<ApiSettingsRoot>,
    safety: ApiSettingsSafety,
    service: ApiSettingsService,
}

#[derive(serde::Serialize)]
struct ApiSettingsManifest {
    /// URL del manifest primario (la oficial de mtkennerly por defecto).
    primary_url: String,
    primary_enabled: bool,
    /// Lista de manifests secundarios. Cada uno es local (path) o
    /// remote (url), con su flag enable.
    secondary: Vec<ApiSettingsSecondaryManifest>,
}

#[derive(serde::Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
enum ApiSettingsSecondaryManifest {
    Local { path: String, enabled: bool },
    Remote { url: String, enabled: bool },
}

#[derive(serde::Serialize)]
struct ApiSettingsRoot {
    /// Nombre del store en lowercase (steam, epic, gog, gogGalaxy,
    /// heroic, legendary, lutris, microsoft, origin, prime, uplay,
    /// otherHome, otherWine, otherWindows, otherLinux, otherMac, other).
    /// Mismo serde-rename que `Store` del manifest.
    store: String,
    path: String,
}

#[derive(serde::Serialize)]
struct ApiSettingsSafety {
    safety_backups_enabled: bool,
    system_notifications_enabled: bool,
}

#[derive(serde::Serialize)]
struct ApiSettingsService {
    /// `true` si la scheduled task / systemd service está instalada.
    /// Sólo Windows por ahora — Linux/Mac todavía no tienen la lógica
    /// de detección portada (la GUI también devuelve `false` ahí).
    installed: bool,
}

async fn settings_handler() -> Json<ApiSettingsResponse> {
    use crate::resource::config::{Config, SecondaryManifestConfig};
    use crate::sync::sync_config::SyncGamesConfig;

    let config = Config::load().unwrap_or_default();
    let sync_cfg = SyncGamesConfig::load();

    // Manifest URL primario: la GUI usa `config.manifest.url()` que cae
    // a la default oficial si está None. Replicamos para que el plugin
    // siempre vea la URL real, no `null`.
    let primary_url = config.manifest.url().to_string();
    let primary_enabled = config.manifest.enable;

    let secondary: Vec<ApiSettingsSecondaryManifest> = config
        .manifest
        .secondary
        .iter()
        .map(|s| match s {
            SecondaryManifestConfig::Local { path, enable } => ApiSettingsSecondaryManifest::Local {
                path: path.render(),
                enabled: *enable,
            },
            SecondaryManifestConfig::Remote { url, enable } => ApiSettingsSecondaryManifest::Remote {
                url: url.clone(),
                enabled: *enable,
            },
        })
        .collect();

    // Roots: aplanamos el enum Root → { store: lowercase_camel, path }.
    // El plugin no necesita distinguir Steam de Epic en el render
    // (todos se muestran igual con label de store).
    fn store_to_str(s: crate::resource::manifest::Store) -> &'static str {
        use crate::resource::manifest::Store;
        match s {
            Store::Ea => "ea",
            Store::Epic => "epic",
            Store::Gog => "gog",
            Store::GogGalaxy => "gogGalaxy",
            Store::Heroic => "heroic",
            Store::Legendary => "legendary",
            Store::Lutris => "lutris",
            Store::Microsoft => "microsoft",
            Store::Origin => "origin",
            Store::Prime => "prime",
            Store::Steam => "steam",
            Store::Uplay => "uplay",
            Store::OtherHome => "otherHome",
            Store::OtherWine => "otherWine",
            Store::OtherWindows => "otherWindows",
            Store::OtherLinux => "otherLinux",
            Store::OtherMac => "otherMac",
            Store::Other => "other",
        }
    }
    let roots: Vec<ApiSettingsRoot> = config
        .roots
        .iter()
        .map(|r| ApiSettingsRoot {
            store: store_to_str(r.store()).to_string(),
            path: r.path().render(),
        })
        .collect();

    Json(ApiSettingsResponse {
        backup_path: config.backup.path.render(),
        manifest: ApiSettingsManifest {
            primary_url,
            primary_enabled,
            secondary,
        },
        roots,
        safety: ApiSettingsSafety {
            safety_backups_enabled: sync_cfg.safety_backups_enabled(),
            system_notifications_enabled: sync_cfg.system_notifications_enabled(),
        },
        service: ApiSettingsService {
            installed: detect_service_installed(),
        },
    })
}

/// Detecta si el daemon está instalado como servicio del SO. Misma
/// lógica que la GUI (src/gui/app.rs:4220) — Windows sólo, los demás
/// SOs reportan false. Cuando portemos la lógica al daemon worker
/// (Fase 2 o posterior) este helper se quita.
fn detect_service_installed() -> bool {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        std::process::Command::new("schtasks.exe")
            .args(["/query", "/TN", "LudusaviDaemon"])
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .map(|out| out.status.success())
            .unwrap_or(false)
    }
    #[cfg(not(target_os = "windows"))]
    {
        false
    }
}

// ============================================================================
// POST /api/settings/safety — toggles SAFETY (Fase 2 — primer write endpoint)
// ============================================================================
//
// Equivale a Message::ToggleSafetyBackupsEnabled y
// Message::ToggleSystemNotificationsEnabled de la GUI Iced. Ambos se
// pueden cambiar en el mismo POST (campos opcionales) o por separado.
//
// Side-effect: rota `sync-games.json` con los nuevos valores. El file
// watcher del daemon HTTP detecta el cambio y emite `daemon_restarted`
// — los clientes (plugin, futuras GUIs) refrescan.
//
// **Patrón establecido aquí** (replicado en todos los POST de Fase 2):
//   1. Extractor `Json<T>` con todos los campos opcionales (PATCH-style:
//      sólo se actualiza lo que viene).
//   2. Cargar la config actual via `SyncGamesConfig::load()` /
//      `Config::load()` etc., aplicar cambios, persistir con `save()`.
//   3. Devolver el estado completo tras el cambio (echo) para que el
//      cliente pueda reconciliar UI sin re-fetch.
//   4. El watcher emite el evento SSE — no lo emitimos nosotros desde
//      el handler. Single source of truth.

#[derive(serde::Deserialize, Debug)]
struct SafetyUpdateBody {
    #[serde(default)]
    safety_backups_enabled: Option<bool>,
    #[serde(default)]
    system_notifications_enabled: Option<bool>,
}

#[derive(serde::Serialize)]
struct SafetyEchoResponse {
    safety_backups_enabled: bool,
    system_notifications_enabled: bool,
}

async fn settings_safety_handler(
    Json(body): Json<SafetyUpdateBody>,
) -> Result<Json<SafetyEchoResponse>, (StatusCode, Json<serde_json::Value>)> {
    use crate::sync::sync_config::SyncGamesConfig;

    // Defensive: si el body viene completamente vacío, no es 400 — sólo
    // un no-op que devuelve el estado actual. Útil como "ping" para
    // confirmar el endpoint.
    let mut cfg = SyncGamesConfig::load();
    if let Some(v) = body.safety_backups_enabled {
        cfg.safety_backups_enabled = v;
    }
    if let Some(v) = body.system_notifications_enabled {
        cfg.system_notifications_enabled = v;
    }
    cfg.save();

    Ok(Json(SafetyEchoResponse {
        safety_backups_enabled: cfg.safety_backups_enabled,
        system_notifications_enabled: cfg.system_notifications_enabled,
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

// ============================================================================
// /api/accela-installs — juegos instalados via ACCELA
// ============================================================================
//
// Spawnea el adapter Python (`accela_adapter/adapter.py`) con el comando
// `list_accela_installs` y devuelve la lista. Si la config de ACCELA no
// está completa (bin_path, python_path) o no encontramos el adapter
// script, devolvemos lista vacía — el plugin sabe gestionar ese caso
// gracefully en lugar de mostrar error.
//
// Es la 4ª fuente de juegos que la GUI Iced ya integra desde Phase 4
// de la integración ACCELA. El plugin Millennium la consume en paralelo
// a /api/games y mergea para mostrar el total al usuario.

#[derive(serde::Serialize, serde::Deserialize, Default)]
struct ApiAccelaInstall {
    #[serde(default)]
    appid: String,
    #[serde(default)]
    game_name: String,
    #[serde(default)]
    install_path: String,
    #[serde(default)]
    library_path: String,
    #[serde(default)]
    size_on_disk: u64,
    #[serde(default)]
    buildid: String,
    #[serde(default)]
    last_updated: String,
    #[serde(default)]
    accela_marker_path: String,
    #[serde(default)]
    appmanifest_path: String,
}

#[derive(serde::Serialize)]
struct ApiAccelaInstallsResponse {
    installs: Vec<ApiAccelaInstall>,
    /// Mensaje opcional cuando devolvemos lista vacía: explica el motivo
    /// (no configurado, adapter no encontrado, error). El plugin lo puede
    /// surface al usuario como hint.
    note: Option<String>,
}

/// Resuelve el path al adapter probando ubicaciones estándar.
fn resolve_adapter_path() -> Option<std::path::PathBuf> {
    let candidates = [
        // 1. Junto al binario del daemon (deployment recomendado).
        std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|p| p.to_path_buf()))
            .map(|p| p.join("accela_adapter").join("adapter.py")),
        // 2. Relativo al cwd (cuando se corre desde el repo, p.ej.
        //    `cargo run --bin ludusavi-daemon` desde la raíz del worktree).
        Some(std::path::PathBuf::from("accela_adapter").join("adapter.py")),
    ];
    for cand in candidates.into_iter().flatten() {
        if cand.exists() {
            return Some(cand);
        }
    }
    None
}

async fn accela_installs_handler() -> Json<ApiAccelaInstallsResponse> {
    use crate::resource::config::Config;

    let config = match Config::load() {
        Ok(c) => c,
        Err(e) => {
            log::warn!("[daemon-http] config load failed for accela: {e:?}");
            return Json(ApiAccelaInstallsResponse {
                installs: vec![],
                note: Some(format!("Config load failed: {e:?}")),
            });
        }
    };

    let bin_path = config.accela.bin_path.trim().to_string();
    let python_path = config.accela.python_path.trim().to_string();

    if bin_path.is_empty() || python_path.is_empty() {
        return Json(ApiAccelaInstallsResponse {
            installs: vec![],
            note: Some(
                "ACCELA paths not configured. Set them in the GUI Iced (Settings → ACCELA).".to_string(),
            ),
        });
    }

    let adapter_path = match resolve_adapter_path() {
        Some(p) => p,
        None => {
            return Json(ApiAccelaInstallsResponse {
                installs: vec![],
                note: Some(
                    "ACCELA adapter not found. Copy `accela_adapter/` next to the daemon binary."
                        .to_string(),
                ),
            });
        }
    };

    match spawn_adapter_list_installs(&python_path, &adapter_path, &bin_path).await {
        Ok(installs) => Json(ApiAccelaInstallsResponse {
            installs,
            note: None,
        }),
        Err(e) => {
            log::warn!("[daemon-http] accela list_installs failed: {e}");
            Json(ApiAccelaInstallsResponse {
                installs: vec![],
                note: Some(format!("Adapter call failed: {e}")),
            })
        }
    }
}

/// Spawnea el adapter, manda `list_accela_installs` por stdin y parsea
/// la respuesta. Mismo patrón que la GUI usa en `accela::send_command`.
async fn spawn_adapter_list_installs(
    python_path: &str,
    adapter_path: &std::path::Path,
    bin_path: &str,
) -> Result<Vec<ApiAccelaInstall>, String> {
    use std::process::Stdio;
    use tokio::io::AsyncWriteExt;

    let mut cmd = tokio::process::Command::new(python_path);
    cmd.arg(adapter_path)
        .arg("--accela-path")
        .arg(bin_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    // CREATE_NO_WINDOW para que en Windows no parpadee una consola.
    // tokio::process::Command expone creation_flags() directamente en
    // Windows; no hay que importar std::os::windows::process::CommandExt.
    #[cfg(target_os = "windows")]
    cmd.creation_flags(0x08000000);

    let mut child = cmd.spawn().map_err(|e| format!("spawn failed: {e}"))?;

    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| "stdin pipe missing".to_string())?;
    stdin
        .write_all(b"{\"cmd\": \"list_accela_installs\"}\n")
        .await
        .map_err(|e| format!("stdin write: {e}"))?;
    drop(stdin);

    let output = child
        .wait_with_output()
        .await
        .map_err(|e| format!("wait_with_output: {e}"))?;

    if !output.status.success() && output.stdout.is_empty() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("adapter exited with {}: {stderr}", output.status));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let line = stdout
        .lines()
        .next()
        .ok_or_else(|| "no output from adapter".to_string())?;
    let event: serde_json::Value =
        serde_json::from_str(line).map_err(|e| format!("json parse: {e} (line: {line})"))?;

    match event.get("event").and_then(|v| v.as_str()) {
        Some("accela_installs") => {
            let games = event
                .get("games")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();
            let parsed: Vec<ApiAccelaInstall> = games
                .into_iter()
                .filter_map(|v| serde_json::from_value(v).ok())
                .collect();
            Ok(parsed)
        }
        Some("error") => Err(event
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown adapter error")
            .to_string()),
        other => Err(format!("unexpected adapter event: {other:?}")),
    }
}

// ============================================================================
// /api/events — Server-Sent Events stream
// ============================================================================
//
// Long-lived stream donde el daemon empuja eventos en tiempo real:
// cuando un juego cambia de status, cuando el worker reinicia, cuando
// rclone se cae, etc. Sustituye el polling actual de la GUI Iced
// (que lee daemon-status.json cada 5s) por un push limpio.
//
// El plugin abre `EventSource('/api/events')` (validado en hello-world A
// 2026-05-06). Cuando recibe un evento, refresca la parte relevante
// de su UI fetcheando el endpoint correspondiente.
//
// La integración con el worker loop (cuándo se emiten eventos
// concretos) llega en commits posteriores. Por ahora el endpoint está
// expuesto y funcional; si nadie llama a `state.events.send(...)` el
// stream sólo emite los keep-alives.

use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use std::convert::Infallible;
use tokio_stream::{wrappers::BroadcastStream, Stream, StreamExt as _};

async fn events_handler(
    State(state): State<AppState>,
) -> Sse<impl Stream<Item = Result<SseEvent, Infallible>>> {
    let receiver = state.events.subscribe();
    let stream = BroadcastStream::new(receiver).filter_map(|result| match result {
        Ok(event) => SseEvent::default()
            .json_data(&event)
            .ok()
            .map(Ok::<_, Infallible>),
        // BroadcastStream emite Lagged cuando el cliente es más lento
        // que la capacidad del canal y se han dropeado eventos. Lo
        // ignoramos en lugar de cerrar el stream — el plugin puede
        // re-fetchear el estado completo via /api/games si necesita
        // recuperarse.
        Err(_lagged) => None,
    });
    // KeepAlive::default() = comentario `:` cada 15s. CEF mantiene la
    // conexión abierta indefinidamente con eso (validado en hello-world A).
    Sse::new(stream).keep_alive(KeepAlive::default())
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
        .route("/api/events", get(events_handler))
        .route("/api/accela-installs", get(accela_installs_handler))
        .route("/api/cloud", get(cloud_handler))
        .route("/api/settings", get(settings_handler))
        .route("/api/settings/safety", post(settings_safety_handler))
        // El middleware de auth se aplica DESPUÉS del cors layer para
        // que las peticiones OPTIONS (preflight) no requieran token.
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            require_auth,
        ))
        .with_state(state)
        .layer(cors)
}

/// Arranca un watcher (notify_debouncer_full) sobre los 3 JSONs que el
/// worker loop escribe — `daemon-status.json`, `ludusavi-game-list.json`,
/// `sync-games.json` — y publica eventos al broadcaster cuando cambian.
///
/// Es la "option B" de la Fase 0: en lugar de inyectar el broadcaster
/// dentro del worker loop (que requeriría cambiar la firma de
/// `start_daemon` y razonar sobre concurrencia entre 2 hilos), vigilamos
/// los efectos en disco que el worker ya produce. Coarse-grained pero
/// suficiente: el plugin re-fetchea el endpoint correspondiente.
///
/// Devuelve el debouncer; mientras vive el watcher sigue activo. Al
/// dropearlo, el thread interno termina.
fn start_event_watcher(
    app_dir: &StrictPath,
    sender: tokio::sync::broadcast::Sender<DaemonEvent>,
) -> Result<
    notify_debouncer_full::Debouncer<
        notify::RecommendedWatcher,
        notify_debouncer_full::FileIdMap,
    >,
    String,
> {
    use notify::{RecursiveMode, Watcher};
    use notify_debouncer_full::{new_debouncer, DebounceEventResult};

    let dir = app_dir
        .as_std_path_buf()
        .map_err(|e| format!("Cannot resolve app_dir: {e:?}"))?;

    // Crea el dir si no existe (puede pasar en primer arranque cuando el
    // worker loop aún no ha escrito nada).
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("Cannot create app_dir {dir:?}: {e}"))?;

    // Debounce 500ms: el daemon a veces escribe el mismo JSON varias
    // veces en ráfaga (write a tempfile + rename). Coalescemos.
    let watcher_sender = sender.clone();
    let mut debouncer = new_debouncer(
        std::time::Duration::from_millis(500),
        None,
        move |result: DebounceEventResult| {
            let events = match result {
                Ok(events) => events,
                Err(errors) => {
                    log::error!("[daemon-http watcher] errors: {errors:?}");
                    return;
                }
            };

            let mut emit_games = false;
            let mut emit_devices = false;
            let mut emit_restart = false;

            for event in events {
                for path in &event.paths {
                    let filename = path
                        .file_name()
                        .and_then(|s| s.to_str())
                        .unwrap_or("");
                    match filename {
                        "ludusavi-game-list.json" => {
                            emit_games = true;
                            emit_devices = true;
                        }
                        "daemon-status.json" => {
                            emit_games = true;
                        }
                        "sync-games.json" => {
                            emit_restart = true;
                        }
                        _ => {} // ignorar daemon.log, daemon-state.json, etc.
                    }
                }
            }

            // broadcast::Sender::send es no-bloqueante; falla con Err si
            // no hay subscribers vivos, lo cual es OK — sólo tirar.
            if emit_games {
                let _ = watcher_sender.send(DaemonEvent::GamesChanged);
            }
            if emit_devices {
                let _ = watcher_sender.send(DaemonEvent::DevicesChanged);
            }
            if emit_restart {
                let _ = watcher_sender.send(DaemonEvent::DaemonRestarted);
            }
        },
    )
    .map_err(|e| format!("Failed to create file watcher: {e}"))?;

    // Vigilamos el directorio entero (NonRecursive) en lugar de los
    // ficheros uno a uno: aguanta cuando los ficheros aún no existen
    // (primer arranque), o cuando el worker hace tempfile + rename.
    debouncer
        .watcher()
        .watch(&dir, RecursiveMode::NonRecursive)
        .map_err(|e| format!("Failed to watch {dir:?}: {e}"))?;
    log::info!("[daemon-http] file watcher active on {dir:?}");

    Ok(debouncer)
}

/// Arranca el servidor HTTP y bloquea hasta que `stop_flag` se active.
/// Llamado desde el binario del daemon en paralelo al worker loop.
pub fn run_http_server(stop_flag: Arc<AtomicBool>) -> Result<(), String> {
    let token = load_or_create_token()?;
    let (events_tx, _) = tokio::sync::broadcast::channel(EVENT_CHANNEL_CAPACITY);

    // Watcher de ficheros. Lo guardamos como `_watcher` (no `_`) para
    // que su Drop se llame al final de esta función, parando el thread
    // interno de notify limpiamente. Si la creación falla loggeamos y
    // seguimos sin SSE — la API sigue siendo útil sin push events.
    let _watcher = match start_event_watcher(&app_dir(), events_tx.clone()) {
        Ok(w) => Some(w),
        Err(e) => {
            log::warn!("[daemon-http] event watcher failed to start, SSE will only emit keep-alives: {e}");
            None
        }
    };

    let state = AppState {
        token: Arc::new(token),
        events: events_tx,
    };

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("daemon-http")
        .build()
        .map_err(|e| format!("Failed to create tokio runtime: {e}"))?;

    runtime.block_on(async move {
        let app = build_router(state);
        let addr = format!("127.0.0.1:{}", DAEMON_HTTP_PORT);
        // Nota sobre TIME_WAIT: en Windows, SO_REUSEADDR tiene semántica
        // distinta (permite que OTRO proceso reuse el puerto, no es
        // reuse-after-close). En Linux sí ayuda con TIME_WAIT pero es
        // raro hitear ese caso. Por ahora bind directo; si reiniciar
        // el daemon falla con EADDRINUSE el usuario puede esperar 30s
        // o reintentar. Veremos si en práctica es necesario más.
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
        let (events, _) = tokio::sync::broadcast::channel(EVENT_CHANNEL_CAPACITY);
        AppState {
            token: Arc::new(token.to_string()),
            events,
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

    /// Auth via `?token=...` query string. Necesario para EventSource
    /// (la API estándar no permite headers custom).
    #[tokio::test]
    async fn status_returns_200_with_correct_query_token() {
        let token = "correct-token-xyz";
        let app = build_router(test_state(token));
        let response = app
            .oneshot(
                Request::builder()
                    .uri(format!("/api/status?token={token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    /// Query token incorrecto = 401. Igual que un Authorization header
    /// equivocado.
    #[tokio::test]
    async fn status_returns_401_with_wrong_query_token() {
        let app = build_router(test_state("right-token"));
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/status?token=wrong-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    /// Si vienen ambos canales, gana el header — preferimos el más
    /// seguro. Aquí el query es válido pero el header no, debería 401.
    #[tokio::test]
    async fn header_token_takes_precedence_over_query_token() {
        let app = build_router(test_state("right-token"));
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/status?token=right-token")
                    .header("Authorization", "Bearer wrong-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    /// El parsing de query tolera otros params junto al token (p.ej.
    /// si más adelante añadimos `?token=X&since=...` para SSE replay).
    #[tokio::test]
    async fn query_token_parsing_tolerates_extra_params() {
        let token = "right-token";
        let app = build_router(test_state(token));
        let response = app
            .oneshot(
                Request::builder()
                    .uri(format!("/api/status?since=42&token={token}&foo=bar"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    // ------------------------------------------------------------------
    // POST /api/settings/safety — primer endpoint write de Fase 2
    // ------------------------------------------------------------------

    /// Mutex que serializa los tests que mutan `CONFIG_DIR` para que
    /// puedan correr en paralelo sin pisarse. Necesario porque
    /// `app_dir()` consulta el global `CONFIG_DIR` y los tests apuntan
    /// a tempdirs distintos.
    static CONFIG_DIR_TEST_LOCK: std::sync::Mutex<()> =
        std::sync::Mutex::new(());

    /// RAII que apunta `CONFIG_DIR` a un tempdir y lo restaura al
    /// dropear. Mantiene un `MutexGuard` para serializar tests que
    /// muten el global.
    struct ConfigDirGuard {
        _lock: std::sync::MutexGuard<'static, ()>,
        prev: Option<std::path::PathBuf>,
        // Mantenemos el TempDir vivo hasta el drop para que el path
        // siga existiendo durante el test.
        _tmp: tempfile::TempDir,
    }

    impl ConfigDirGuard {
        fn new(tmp: tempfile::TempDir) -> Self {
            // Si el test fallara dejando el lock envenenado, lo
            // recuperamos con `into_inner` — este lock sólo serializa,
            // no protege state crítico.
            let lock = CONFIG_DIR_TEST_LOCK
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            let prev = crate::prelude::CONFIG_DIR
                .lock()
                .unwrap()
                .replace(tmp.path().to_path_buf());
            Self {
                _lock: lock,
                prev,
                _tmp: tmp,
            }
        }
    }

    impl Drop for ConfigDirGuard {
        fn drop(&mut self) {
            *crate::prelude::CONFIG_DIR.lock().unwrap() = self.prev.take();
        }
    }

    #[tokio::test]
    async fn settings_safety_post_requires_auth() {
        let app = build_router(test_state("token"));
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/settings/safety")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"safety_backups_enabled":false}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn settings_safety_post_persists_both_fields() {
        let _guard = ConfigDirGuard::new(tempfile::tempdir().unwrap());
        let token = "tok";
        let app = build_router(test_state(token));

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/settings/safety")
                    .header("Authorization", format!("Bearer {token}"))
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"safety_backups_enabled":false,"system_notifications_enabled":false}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        // Echo response refleja el nuevo estado.
        let bytes = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["safety_backups_enabled"], false);
        assert_eq!(json["system_notifications_enabled"], false);

        // Persistido a disco — al reload tiene los nuevos valores.
        let cfg = crate::sync::sync_config::SyncGamesConfig::load();
        assert!(!cfg.safety_backups_enabled);
        assert!(!cfg.system_notifications_enabled);
    }

    #[tokio::test]
    async fn settings_safety_post_partial_update_preserves_other_field() {
        let _guard = ConfigDirGuard::new(tempfile::tempdir().unwrap());
        let token = "tok";
        let app = build_router(test_state(token));

        // Primer POST: ambos a false.
        let _ = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/settings/safety")
                    .header("Authorization", format!("Bearer {token}"))
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"safety_backups_enabled":false,"system_notifications_enabled":false}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        // Segundo POST: sólo safety_backups a true. system_notifications
        // debe quedarse en false (no tocado).
        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/settings/safety")
                    .header("Authorization", format!("Bearer {token}"))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"safety_backups_enabled":true}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let cfg = crate::sync::sync_config::SyncGamesConfig::load();
        assert!(cfg.safety_backups_enabled);
        assert!(!cfg.system_notifications_enabled);
    }

    #[tokio::test]
    async fn settings_safety_post_empty_body_is_noop() {
        let _guard = ConfigDirGuard::new(tempfile::tempdir().unwrap());
        let token = "tok";
        let app = build_router(test_state(token));

        let response = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/settings/safety")
                    .header("Authorization", format!("Bearer {token}"))
                    .header("content-type", "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        // Estado por defecto preservado.
        let cfg = crate::sync::sync_config::SyncGamesConfig::load();
        assert!(cfg.safety_backups_enabled);
        assert!(cfg.system_notifications_enabled);
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
    async fn events_endpoint_returns_200_and_event_stream_content_type() {
        let token = "tok";
        let app = build_router(test_state(token));
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/events")
                    .header("Authorization", format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let ct = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        assert!(
            ct.contains("text/event-stream"),
            "expected SSE content-type, got: {ct}"
        );
    }

    #[tokio::test]
    async fn events_endpoint_delivers_published_events() {
        // Estado compartido entre el cliente HTTP y el publisher: el
        // mismo state — porque AppState::clone duplica el Arc del
        // sender y comparten el mismo canal de broadcast.
        let token = "tok";
        let state = test_state(token);
        let publisher = state.events.clone();
        let app = build_router(state);

        // Lanzamos la request y, en paralelo, publicamos un evento
        // tras un pequeño delay para asegurar que el subscriber esté
        // listo. (Si publicamos antes de que oneshot llegue al
        // handler, broadcast::send() no encuentra subscribers y se
        // pierde silenciosamente.)
        let publish = tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            // ignoramos send result: si nadie esta escuchando, broadcast
            // devuelve un Err transitorio que no nos importa aqui.
            let _ = publisher.send(DaemonEvent::DaemonRestarted);
        });

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/events")
                    .header("Authorization", format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        // Leemos el body en chunks hasta encontrar el JSON del evento
        // o agotar 2s (timeout duro para que el test no se cuelgue si
        // algo va mal).
        use http_body_util::BodyExt;
        let mut body = response.into_body();
        let mut accumulated = Vec::new();
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
        let found = loop {
            if tokio::time::Instant::now() >= deadline {
                break false;
            }
            let frame = tokio::time::timeout(
                std::time::Duration::from_millis(100),
                body.frame(),
            )
            .await;
            match frame {
                Ok(Some(Ok(frame))) => {
                    if let Some(data) = frame.data_ref() {
                        accumulated.extend_from_slice(data);
                        if std::str::from_utf8(&accumulated)
                            .map(|s| s.contains("daemon_restarted"))
                            .unwrap_or(false)
                        {
                            break true;
                        }
                    }
                }
                Ok(Some(Err(_))) | Ok(None) => break false,
                Err(_) => continue, // timeout del frame, intenta otra vez
            }
        };

        publish.await.ok();
        assert!(
            found,
            "did not see the published event in the SSE body within 2s. accumulated={}",
            String::from_utf8_lossy(&accumulated)
        );
    }

    #[tokio::test]
    async fn file_watcher_emits_games_changed_when_status_json_written() {
        let tmp = tempfile::tempdir().unwrap();
        let (sender, mut rx) = tokio::sync::broadcast::channel(8);
        let _watcher = start_event_watcher(&sp(tmp.path()), sender).unwrap();
        // Pequeño margen para que el watcher empiece a observar.
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        std::fs::write(tmp.path().join("daemon-status.json"), b"{}").unwrap();

        // Debounce 500ms + margen.
        let event = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
            .await
            .expect("watcher should emit within 2s")
            .expect("broadcast not closed");
        assert_eq!(event, DaemonEvent::GamesChanged);
    }

    #[tokio::test]
    async fn file_watcher_emits_both_games_and_devices_when_game_list_written() {
        let tmp = tempfile::tempdir().unwrap();
        let (sender, mut rx) = tokio::sync::broadcast::channel(8);
        let _watcher = start_event_watcher(&sp(tmp.path()), sender).unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        std::fs::write(
            tmp.path().join("ludusavi-game-list.json"),
            br#"{"games":[],"device_names":{}}"#,
        )
        .unwrap();

        // Esperamos al debounce (500ms) + margen, y después drenamos
        // todos los eventos pendientes.
        tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
        let mut got = Vec::new();
        while let Ok(Ok(ev)) =
            tokio::time::timeout(std::time::Duration::from_millis(50), rx.recv()).await
        {
            got.push(ev);
        }

        assert!(
            got.contains(&DaemonEvent::GamesChanged),
            "expected GamesChanged in {got:?}"
        );
        assert!(
            got.contains(&DaemonEvent::DevicesChanged),
            "expected DevicesChanged in {got:?}"
        );
    }

    #[tokio::test]
    async fn file_watcher_emits_daemon_restarted_when_sync_games_written() {
        let tmp = tempfile::tempdir().unwrap();
        let (sender, mut rx) = tokio::sync::broadcast::channel(8);
        let _watcher = start_event_watcher(&sp(tmp.path()), sender).unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        std::fs::write(
            tmp.path().join("sync-games.json"),
            br#"{"games":{},"safety_backups_enabled":true,"system_notifications_enabled":true}"#,
        )
        .unwrap();

        let event = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
            .await
            .expect("watcher should emit within 2s")
            .expect("broadcast not closed");
        assert_eq!(event, DaemonEvent::DaemonRestarted);
    }

    #[tokio::test]
    async fn file_watcher_ignores_unrelated_files() {
        let tmp = tempfile::tempdir().unwrap();
        let (sender, mut rx) = tokio::sync::broadcast::channel(8);
        let _watcher = start_event_watcher(&sp(tmp.path()), sender).unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // Ficheros no monitorizados — daemon.log, daemon-state.json,
        // ludusavi-device.json — no deben disparar eventos.
        std::fs::write(tmp.path().join("daemon.log"), b"random log line").unwrap();
        std::fs::write(tmp.path().join("daemon-state.json"), b"{}").unwrap();
        std::fs::write(
            tmp.path().join("ludusavi-device.json"),
            br#"{"id":"x","name":"y"}"#,
        )
        .unwrap();

        // Esperamos 1s — más que el debounce — y comprobamos que no
        // llegó nada.
        let attempt = tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv()).await;
        assert!(
            attempt.is_err(),
            "no event expected, got: {attempt:?}"
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
