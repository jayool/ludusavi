//! Notificaciones nativas del sistema operativo.
//!
//! Por ahora solo Windows (toast del Action Center).
//! macOS/Linux son no-op para no romper la build; se implementarán más adelante.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotificationLevel {
    Info,
    Warning,
    Error,
}

/// Muestra una notificación nativa del SO.
/// En plataformas no soportadas, es no-op (solo loguea).
#[cfg(target_os = "windows")]
pub fn show_notification(title: &str, body: &str, _level: NotificationLevel) {
    use tauri_winrt_notification::{Toast, Duration as ToastDuration};

    log::info!("[system-notifications] Attempting to show: {} - {}", title, body);

    // Usamos el AppID de PowerShell que YA existe registrado en todos los Windows.
    // Esto hace que la notificación aparezca en el Action Center.
    // El downside: la notificación dirá "Windows PowerShell" como app emisora.
    // Para que dijera "Ludusavi" haría falta registrar nuestro propio AppUserModelID
    // en el registro de Windows (típicamente desde un instalador).
    let app_id = Toast::POWERSHELL_APP_ID;

    let result = Toast::new(app_id)
        .title(title)
        .text1(body)
        .duration(ToastDuration::Short)
        .show();

    match result {
        Ok(_) => log::info!("[system-notifications] Notification shown successfully"),
        Err(e) => log::warn!("[system-notifications] Failed to show notification: {:?}", e),
    }
}

#[cfg(not(target_os = "windows"))]
pub fn show_notification(title: &str, body: &str, _level: NotificationLevel) {
    log::debug!(
        "[system-notifications] Suppressed (platform not supported): {} - {}",
        title,
        body
    );
}
