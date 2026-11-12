//! Background daemon entry point (no console window on Windows).
#![cfg_attr(windows, windows_subsystem = "windows")]

fn main() {
    if let Err(e) = openflow::daemon::run(false) {
        log::error!("fatal: {e:#}");
        std::process::exit(1);
    }
}
