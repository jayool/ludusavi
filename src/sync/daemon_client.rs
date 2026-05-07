/*!
Cliente HTTP del daemon, para uso desde la GUI Iced.

Mismo daemon HTTP que el plugin Millennium consume — pero el GUI corre
en el mismo proceso/host que el daemon, así que las llamadas son a
`localhost:61234`. Tiene token auth como cualquier otro cliente, leído
del fichero canónico `app_dir/daemon-token.txt`.

Por qué la GUI llama HTTP en vez de escribir JSONs directamente:

> "Migración incremental: cada vez que añadamos un POST nuevo, la GUI
>  lo usa también en lugar de escribir JSON. Single source of truth
>  = el daemon."  (anexo I al draft Millennium/Decky, decisión a)

El daemon worker tiene file watcher así que el dual-write GUI+daemon
era tolerable, pero la migración elimina race conditions y centraliza
validación. Cada nuevo endpoint POST en `daemon_http.rs` que la GUI
necesite reproducir aquí.

Esta módulo se mantiene minimalista a propósito — sólo wrappers
async finitos que las Tasks de Iced pueden invocar. Sin singletons,
sin estado global. El token se lee fresh en cada llamada (es barato,
~60 bytes de un fichero local) — evita bug class si rota.
*/

use crate::prelude::app_dir;

/// Ruta hardcodeada del daemon HTTP en localhost. Mismo puerto que
/// `DAEMON_HTTP_PORT` en daemon_http.rs — duplicado a propósito para
/// que este módulo no tenga dependencia de ese (evita ciclo de
/// import; el daemon binario no necesita el cliente).
const DAEMON_BASE_URL: &str = "http://localhost:61234";

/// Lee el token del daemon de `app_dir/daemon-token.txt`. Devuelve
/// error si el daemon nunca ha corrido (no hay fichero) o no es
/// legible. Igual que la convención del plugin Millennium.
fn read_token() -> Result<String, String> {
    let path = app_dir().joined("daemon-token.txt");
    let content = path
        .read()
        .ok_or_else(|| "Daemon token file not found — daemon nunca arrancado?".to_string())?;
    let trimmed = content.trim().to_string();
    if trimmed.is_empty() {
        return Err("Daemon token file empty".to_string());
    }
    Ok(trimmed)
}

/// Echo de la respuesta del endpoint POST /api/settings/safety. Misma
/// shape que `SafetyEchoResponse` del daemon HTTP — definida aquí en
/// vez de importarse para mantener este módulo self-contained.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct SafetyEcho {
    pub safety_backups_enabled: bool,
    pub system_notifications_enabled: bool,
}

/// POST /api/settings/safety con body PATCH-style: ambos campos
/// opcionales, sólo se actualiza lo que viene. Devuelve el estado
/// completo tras el cambio para que la GUI reconcilie su in-memory
/// `sync_games_config` sin re-leer el fichero.
pub async fn post_safety(
    safety_backups_enabled: Option<bool>,
    system_notifications_enabled: Option<bool>,
) -> Result<SafetyEcho, String> {
    let token = read_token()?;
    let body = serde_json::json!({
        "safety_backups_enabled": safety_backups_enabled,
        "system_notifications_enabled": system_notifications_enabled,
    });

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{DAEMON_BASE_URL}/api/settings/safety"))
        .bearer_auth(&token)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("HTTP request failed: {e}"))?;

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("HTTP {status}: {body}"));
    }

    resp.json::<SafetyEcho>()
        .await
        .map_err(|e| format!("Failed to parse response JSON: {e}"))
}

/// Echo del endpoint POST /api/games/{name}/mode. `mode` es el wire
/// format camelCase del enum `SaveMode`.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct GameModeEcho {
    pub name: String,
    pub mode: crate::sync::sync_config::SaveMode,
    pub auto_sync: bool,
}

/// POST /api/games/{name}/mode — cambia el save mode del juego.
/// Devuelve el echo con name, mode y auto_sync (preservado).
pub async fn post_game_mode(
    name: String,
    mode: crate::sync::sync_config::SaveMode,
) -> Result<GameModeEcho, String> {
    let token = read_token()?;
    let body = serde_json::json!({ "mode": mode });
    let url = format!(
        "{DAEMON_BASE_URL}/api/games/{}/mode",
        urlencoding_encode(&name),
    );

    let client = reqwest::Client::new();
    let resp = client
        .post(url)
        .bearer_auth(&token)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("HTTP request failed: {e}"))?;

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("HTTP {status}: {body}"));
    }

    resp.json::<GameModeEcho>()
        .await
        .map_err(|e| format!("Failed to parse response JSON: {e}"))
}

/// URL-encode minimal para path components. No traemos la crate
/// `urlencoding` para no inflar dependencies — los nombres de juegos
/// pueden contener espacios, dos puntos, paréntesis, etc., todos
/// caracteres reservados en URL paths que necesitan percent-encoding.
fn urlencoding_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        // Set seguro en path component: A-Z, a-z, 0-9, '-', '_', '.', '~'.
        // Cualquier otro byte se percent-encodea.
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}
