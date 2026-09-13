//! Session display environment preserved across the GUI backend switch.
//!
//! On Linux the window may run under XWayland so hide-to-tray works, while
//! screen capture (portal/PipeWire) needs the user's original session
//! variables. This keeps them without spawning any other application.

#[cfg(target_os = "linux")]
use std::sync::OnceLock;

#[cfg(target_os = "linux")]
static PRESERVED_DISPLAY_ENV: OnceLock<Vec<(String, String)>> = OnceLock::new();

/// Save the given environment variables before the GUI strips them.
#[cfg(target_os = "linux")]
pub fn preserve_display_env(keys: &[&str]) {
    let _ = PRESERVED_DISPLAY_ENV.set(
        keys.iter()
            .filter_map(|key| {
                std::env::var(key)
                    .ok()
                    .map(|value| ((*key).to_string(), value))
            })
            .collect(),
    );
}

/// Saved variables, if any.
#[cfg(target_os = "linux")]
pub fn preserved() -> Option<Vec<(String, String)>> {
    PRESERVED_DISPLAY_ENV.get().cloned()
}

#[cfg(not(target_os = "linux"))]
pub fn preserve_display_env(_keys: &[&str]) {}

/// Saved variables, if any. `None` where the backend switch does not apply.
#[cfg(not(target_os = "linux"))]
pub fn preserved() -> Option<Vec<(String, String)>> {
    None
}
