use std::sync::{atomic::AtomicBool, Arc};

use ludusavi::sync::daemon::{start_daemon, DaemonConfig};

fn main() {
    // Inicializa el logger
    env_logger::Builder::new()
        .filter_level(log::LevelFilter::Info)
        .parse_env("LUDUSAVI_LOG")
        .init();

    log::info!("ludusavi-daemon starting");

    // Flag para detener el daemon limpiamente
    // Equivalente al CancellationToken de EmuSync
    let stop_flag = Arc::new(AtomicBool::new(false));

    // Registra el handler de SIGINT/SIGTERM para parar limpiamente
    // Equivalente a StopAsync en EmuSync
    {
        let stop_flag = stop_flag.clone();
        ctrlc::set_handler(move || {
            log::info!("ludusavi-daemon received stop signal");
            stop_flag.store(true, std::sync::atomic::Ordering::Relaxed);
        })
        .expect("Error setting Ctrl-C handler");
    }

    let config = DaemonConfig::default();
    let handle = start_daemon(stop_flag.clone(), config);

    // Espera a que el daemon termine
    handle.join().expect("Daemon thread panicked");

    log::info!("ludusavi-daemon stopped");
}
