#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(windows)]
mod app;
#[cfg(windows)]
mod integration;

#[cfg(windows)]
fn main() {
    if let Err(error) = app::run() {
        eprintln!("TRend UI lab error: {error}");
    }
}

#[cfg(not(windows))]
fn main() {
    eprintln!("The TRend UI lab currently requires Windows Media Foundation and Win32 GDI.");
}
