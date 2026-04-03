#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use std::sync::{atomic::AtomicBool, Arc};
use ludusavi::sync::daemon::{start_daemon, DaemonConfig};

fn main() {
    let log_path = ludusavi::prelude::app_dir().joined("daemon.log");
    let log_path_str = log_path.as_std_path_buf()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| "daemon.log".to_string());

    flexi_logger::Logger::try_with_env_or_str("info")
        .unwrap_or_else(|_| flexi_logger::Logger::try_with_str("info").unwrap())
        .log_to_file(
            flexi_logger::FileSpec::default()
                .directory(std::path::Path::new(&log_path_str).parent().unwrap_or(std::path::Path::new(".")))
                .basename("daemon")
                .suffix("log")
                .suppress_timestamp(),
        )
        .duplicate_to_stdout(flexi_logger::Duplicate::All)
        .format(flexi_logger::detailed_format)
        .start()
        .unwrap_or_else(|_| {
            env_logger::Builder::new()
                .filter_level(log::LevelFilter::Info)
                .init();
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
fn windows_service_main(arguments: Vec<std::ffi::OsString>) {
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

    use windows_service::service::{
        ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus,
    };

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
    let config = DaemonConfig::default();
    let handle = start_daemon(stop_flag, config);
    handle.join().expect("Daemon thread panicked");
}
