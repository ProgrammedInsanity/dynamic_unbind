use log::{error, info};
use std::path::Path;
use std::{io::stderr, path::PathBuf};
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::{Registry, fmt, prelude::*};

use crate::LibvirtArgs;

fn create_log_file(args: &LibvirtArgs, vm_uuid: &str) -> (std::fs::File, PathBuf) {
    use chrono::Utc;
    let log_path = Path::new("/var/log/dynamic_unbind")
        .join(format!("{}-{}", args.object, vm_uuid))
        .join(format!("{}-{}", args.operation, args.sub_operation));
    let logfile_path = log_path.join(format!("{}.log", Utc::now().format("%Y-%m-%d_%H-%M-%S")));

    let _ = std::fs::create_dir_all(&log_path);

    (
        std::fs::File::create(&logfile_path)
            .unwrap_or_else(|_| panic!("Failed to create log file: {}", &logfile_path.display())),
        logfile_path,
    )
}

#[tracing::instrument(skip_all)]
pub fn setup_logging(args: &LibvirtArgs, vm_uuid: &str) {
    use std::panic;

    let (log_file, log_file_path) = create_log_file(args, vm_uuid);

    let fmt_layer = fmt::layer()
        .with_target(false)
        .with_line_number(true)
        .with_file(true)
        .with_writer(stderr)
        .with_filter(LevelFilter::DEBUG);

    let fmt_layer_file = fmt::layer()
        .with_target(false)
        .with_line_number(true)
        .with_file(true)
        .with_writer(log_file)
        .with_filter(LevelFilter::DEBUG);

    Registry::default()
        .with(fmt_layer)
        .with(
            tracing_journald::layer()
                .expect("Failed to initialize journald tracing layer")
                .with_filter(LevelFilter::DEBUG),
        )
        .with(fmt_layer_file)
        .init();

    let default_hook = panic::take_hook();
    panic::set_hook(Box::new(move |panic_info| {
        let location = panic_info
            .location()
            .map(|loc| format!("{}:{}", loc.file(), loc.line()))
            .unwrap_or_else(|| "<unknown>".to_string());

        let payload = panic_info.payload();
        let msg = if let Some(s) = payload.downcast_ref::<&str>() {
            *s
        } else if let Some(s) = payload.downcast_ref::<String>() {
            s.as_str()
        } else {
            "<non-string panic payload>"
        };

        error!("panic occurred at {location}:\n{msg}");

        default_hook(panic_info);
    }));

    use sha2::{Digest, Sha256};
    use std::fs;

    let binary_hash = Sha256::digest(fs::read("/proc/self/exe").unwrap());
    let binary_hash = hex::encode(binary_hash);
    info!("Log file: {}", log_file_path.display());
    info!("binary hash: {}", binary_hash);
}
