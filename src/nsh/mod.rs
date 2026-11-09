//! Pure Rust NSH entry and shared foundation. The GPUI application remains a separate package.
pub mod foundation;
pub mod shell;

/// A root with the application's embedded fonts. No installed fonts or current directory required.
pub fn root() -> nsh_engine::Root {
    foundation::fonts::install(nsh_engine::Root::of::<shell::Foundation>())
        .app_id("dev.githux.native-shell")
}

#[cfg(test)]
mod tests;
