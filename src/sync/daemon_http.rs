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
