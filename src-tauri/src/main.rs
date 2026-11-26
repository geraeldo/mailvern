#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // Subprocess worker mode for isolated PDF/Office extraction: runs before Tauri/AppKit init
    // so no Dock icon appears, panics/aborts never affect Mailvern, and peak RAM returns to the OS.
    if std::env::args_os().nth(1).is_some_and(|a| a == "--extract-worker") {
        std::process::exit(mail_core::extract::run_worker_stdio());
    }
    mailvern_lib::run()
}
