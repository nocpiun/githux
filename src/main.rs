//! Desktop executable. Application startup owns framework initialization.

#![cfg_attr(all(target_os = "windows", not(test)), windows_subsystem = "windows")]

fn main() -> std::process::ExitCode {
    githux::app::run()
}
