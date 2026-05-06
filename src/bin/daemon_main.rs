#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use ludusavi::sync::daemon::{start_daemon, DaemonConfig};
use std::sync::{atomic::AtomicBool, Arc};

fn main() {
    let log_path = ludusavi::prelude::app_dir().joined("daemon.log");
    let log_path_str = log_path
        .as_std_path_buf()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| "daemon.log".to_string());

    flexi_logger::Logger::try_with_env_or_str("info")
        .unwrap_or_else(|_| flexi_logger::Logger::try_with_str("info").unwrap())
        .log_to_file(
            flexi_logger::FileSpec::default()
                .directory(
                    std::path::Path::new(&log_path_str)
                        .parent()
                        .unwrap_or(std::path::Path::new(".")),
                )
                .basename("daemon")
                .suffix("log")
                .suppress_timestamp(),
        )
        .rotate(
            flexi_logger::Criterion::Size(1024 * 1024 * 5),
            flexi_logger::Naming::Numbers,
            flexi_logger::Cleanup::KeepLogFiles(3),
        )
        .duplicate_to_stdout(flexi_logger::Duplicate::All)
        .format(flexi_logger::detailed_format)
        .start()
        .unwrap_or_else(|_| {
            env_logger::Builder::new().filter_level(log::LevelFilter::Info).init();
            // flexi_logger failed, falling back to env_logger (stdout only)
            panic!("unreachable")
        });

    log::info!("ludusavi-daemon starting");

    run();

    log::info!("ludusavi-daemon stopped");
}

#[cfg(target_os = "windows")]
fn run() {
    // En Windows intentamos correr como servicio.
    // Si falla (porque lo estamos ejecutando manualmente desde terminal)
    // corremos en modo normal.
    if windows_service::service_dispatcher::start("ludusavi-daemon", ffi_service_main).is_err() {
        run_normal();
    }
}

#[cfg(target_os = "windows")]
windows_service::define_windows_service!(ffi_service_main, windows_service_main);

#[cfg(target_os = "windows")]
fn windows_service_main(_arguments: Vec<std::ffi::OsString>) {
    use windows_service::{
        service::ServiceControl,
        service_control_handler::{self, ServiceControlHandlerResult},
    };

    let stop_flag = Arc::new(AtomicBool::new(false));
    let stop_flag_handler = stop_flag.clone();

    let event_handler = move |control_event| -> ServiceControlHandlerResult {
        match control_event {
            ServiceControl::Stop | ServiceControl::Shutdown => {
                log::info!("Windows service stop requested");
                stop_flag_handler.store(true, std::sync::atomic::Ordering::Relaxed);
                ServiceControlHandlerResult::NoError
            }
            ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
            _ => ServiceControlHandlerResult::NotImplemented,
        }
    };

    let status_handle = match service_control_handler::register("ludusavi-daemon", event_handler) {
        Ok(h) => h,
        Err(e) => {
            log::error!("Failed to register service control handler: {e}");
            return;
        }
    };

    use windows_service::service::{ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus};

    // Notifica a Windows que el servicio está corriendo
    let _ = status_handle.set_service_status(ServiceStatus {
        service_type: windows_service::service::ServiceType::OWN_PROCESS,
        current_state: ServiceState::Running,
        controls_accepted: ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN,
        exit_code: ServiceExitCode::Win32(0),
        checkpoint: 0,
        wait_hint: std::time::Duration::default(),
        process_id: None,
    });

    run_with_flag(stop_flag);

    // Notifica a Windows que el servicio se ha detenido
    let _ = status_handle.set_service_status(ServiceStatus {
        service_type: windows_service::service::ServiceType::OWN_PROCESS,
        current_state: ServiceState::Stopped,
        controls_accepted: ServiceControlAccept::empty(),
        exit_code: ServiceExitCode::Win32(0),
        checkpoint: 0,
        wait_hint: std::time::Duration::default(),
        process_id: None,
    });
}

#[cfg(not(target_os = "windows"))]
fn run() {
    // En Linux/SteamOS el propio systemd gestiona el ciclo de vida.
    // Solo necesitamos manejar SIGTERM y SIGINT.
    run_normal();
}

fn run_normal() {
    let stop_flag = Arc::new(AtomicBool::new(false));

    let stop_flag_ctrlc = stop_flag.clone();
    ctrlc::set_handler(move || {
        log::info!("Stop signal received");
        stop_flag_ctrlc.store(true, std::sync::atomic::Ordering::Relaxed);
    })
    .expect("Error setting stop signal handler");

    run_with_flag(stop_flag);
}

fn run_with_flag(stop_flag: Arc<AtomicBool>) {
    let config = DaemonConfig;

    // Worker loop del daemon (file watcher + sync worker) en su hilo.
    let worker_handle = start_daemon(stop_flag.clone(), config);

    // HTTP server (Capa 0 del plan Millennium/Decky) en otro hilo.
    // Bloquea internamente hasta que stop_flag se activa.
    let http_stop = stop_flag.clone();
    let http_handle = std::thread::Builder::new()
        .name("daemon-http-server".into())
        .spawn(move || {
            if let Err(e) = ludusavi::sync::daemon_http::run_http_server(http_stop) {
                log::error!("HTTP server failed: {e}");
            }
        })
        .expect("Failed to spawn HTTP server thread");

    // Esperamos a ambos hilos. Cualquiera que termine antes (worker
    // saliendo por NoCloudConfig, HTTP server crasheando, etc.) debe
    // arrastrar al otro al cerrarse — si no, el proceso se quedaría
    // colgado para siempre. Por eso al hacer join del primero,
    // forzamos stop_flag para que el segundo también cierre.
    if let Err(e) = worker_handle.join() {
        log::error!("Daemon thread panicked: {e:?}");
    }
    log::info!("Worker thread joined; signalling HTTP server to stop");
    stop_flag.store(true, std::sync::atomic::Ordering::Relaxed);
    if let Err(e) = http_handle.join() {
        log::error!("HTTP server thread panicked: {e:?}");
    }
}
