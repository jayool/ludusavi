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
    use notify_rust::Notification;

    log::info!("[system-notifications] Attempting to show: {} - {}", title, body);

    let mut n = Notification::new();
    n.summary(title)
        .body(body)
        .appname("Ludusavi")
        .timeout(notify_rust::Timeout::Milliseconds(5000));

    n.app_id("Ludusavi.SaveSync");

    match n.show() {
        Ok(_) => log::info!("[system-notifications] Notification shown successfully"),
        Err(e) => log::warn!("[system-notifications] Failed to show notification: {}", e),
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
