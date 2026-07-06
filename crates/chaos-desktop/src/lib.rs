//! Tauri shell: loads the bundled web UI and tells it where the server is.
//!
//! The UI resolves its API base from `window.CHAOS_API_BASE` first (see
//! chaos-web/src/main.rs); the shell injects it before the bundle runs. The
//! address comes from, in order: the `CHAOS_SERVER` env var (desktop dev),
//! `$XDG_CONFIG_HOME/chaos/server` (one line, desktop), or nothing — then
//! the UI's own resolution takes over (localStorage override set through
//! the in-app connect screen, which is the path on Android).

use tauri::{WebviewUrl, WebviewWindowBuilder};

fn configured_server() -> Option<String> {
    if let Ok(url) = std::env::var("CHAOS_SERVER") {
        return Some(url.trim().to_string());
    }
    let config = dirs_config()?.join("chaos/server");
    let raw = std::fs::read_to_string(config).ok()?;
    let url = raw.trim();
    (!url.is_empty()).then(|| url.to_string())
}

fn dirs_config() -> Option<std::path::PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".config")))
}

/// Open a link with the system handler — the default browser, or whatever
/// app registered the URL. Outbound links must not navigate the webview.
/// (Android doesn't route through here: the Kotlin `ChaosAndroid.openUrl`
/// bridge fires a VIEW intent instead.)
#[tauri::command]
fn open_external(url: String) -> Result<(), String> {
    let parsed = url::Url::parse(&url).map_err(|e| e.to_string())?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err("only http(s) links open externally".into());
    }
    open_with_system(&url).map_err(|e| e.to_string())
}

#[cfg(target_os = "linux")]
fn open_with_system(url: &str) -> std::io::Result<()> {
    std::process::Command::new("xdg-open")
        .arg(url)
        .spawn()
        .map(|_| ())
}

#[cfg(target_os = "macos")]
fn open_with_system(url: &str) -> std::io::Result<()> {
    std::process::Command::new("open")
        .arg(url)
        .spawn()
        .map(|_| ())
}

#[cfg(windows)]
fn open_with_system(url: &str) -> std::io::Result<()> {
    // ShellExecute semantics without `cmd /C start`'s quoting pitfalls.
    std::process::Command::new("rundll32")
        .args(["url.dll,FileProtocolHandler", url])
        .spawn()
        .map(|_| ())
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn open_with_system(_url: &str) -> std::io::Result<()> {
    Err(std::io::Error::other("no system opener on this platform"))
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // WebKitGTK's DMABUF renderer draws a blank window on the NVIDIA
    // driver; disable it there unless the user decided themselves.
    #[cfg(target_os = "linux")]
    if std::path::Path::new("/proc/driver/nvidia").exists()
        && std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_none()
    {
        unsafe { std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1") };
    }

    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![open_external])
        .setup(|app| {
            let platform = if cfg!(target_os = "android") {
                "android"
            } else {
                "desktop"
            };
            let mut window =
                WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
                    .title("chaos")
                    .inner_size(1280.0, 800.0)
                    .initialization_script(format!("window.CHAOS_PLATFORM = '{platform}';"));
            if let Some(server) = configured_server().filter(|s| url::Url::parse(s).is_ok()) {
                // The URL was just validated; escape quotes anyway.
                let escaped = server.replace('\\', "\\\\").replace('\'', "\\'");
                window =
                    window.initialization_script(format!("window.CHAOS_API_BASE = '{escaped}';"));
            }
            window.build()?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running chaos shell");
}
