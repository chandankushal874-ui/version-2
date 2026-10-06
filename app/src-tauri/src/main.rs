//! main.rs â€” Tauri entrypoint.
//!
//! Spawns the Tokio runtime in a dedicated thread (so the WebView main thread
//! is never blocked), wires Tauri commands to the call state machine, and
//! brings up the UI.

mod audio;
mod prefs;
mod protocol;
mod state;
mod ws;

use state::AppState;
use tauri::Manager;
use tracing_subscriber::EnvFilter;

static RELAY_CHILD: std::sync::Mutex<Option<std::process::Child>> = std::sync::Mutex::new(None);

fn try_spawn_local_relay() {
    // By default, Ollalink Translate connects directly to the Cloud Relay on Render.
    // Local Node.js relay is only spawned if explicitly requested via environment variable:
    // OLLALINK_LOCAL_RELAY=1
    if std::env::var("OLLALINK_LOCAL_RELAY").unwrap_or_default() != "1" {
        return;
    }

    if std::net::TcpStream::connect("127.0.0.1:8787").is_ok() {
        tracing::info!("Local relay already running on port 8787");
        return;
    }

    let mut search_dirs = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            search_dirs.push(dir.to_path_buf());
            if let Some(parent) = dir.parent() {
                search_dirs.push(parent.to_path_buf());
                if let Some(gp) = parent.parent() {
                    search_dirs.push(gp.to_path_buf());
                }
            }
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        search_dirs.push(cwd.clone());
        if let Some(parent) = cwd.parent() {
            search_dirs.push(parent.to_path_buf());
            if let Some(grandparent) = parent.parent() {
                search_dirs.push(grandparent.to_path_buf());
            }
        }
    }

    let rel_candidates = [
        "server/src/server.js",
        "dist-package/server/src/server.js",
        "src/server.js",
    ];

    for base in &search_dirs {
        for rel in &rel_candidates {
            let server_js = base.join(rel);
            if server_js.exists() {
                if let Some(server_root) = server_js.parent().and_then(|p| p.parent()) {
                    let node_candidates = [
                        base.join("node.exe"),
                        base.join("server").join("node.exe"),
                        server_root.join("node.exe"),
                        std::path::PathBuf::from(r"C:\Program Files\nodejs\node.exe"),
                        std::path::PathBuf::from(r"C:\Program Files (x86)\nodejs\node.exe"),
                        std::path::PathBuf::from("node"),
                    ];

                    let mut node_bin = std::path::PathBuf::from("node");
                    for cand in &node_candidates {
                        if cand.exists() {
                            node_bin = cand.clone();
                            break;
                        }
                    }

                    let mut cmd = std::process::Command::new(&node_bin);
                    let env_file = server_root.join(".env");
                    if env_file.exists() {
                        cmd.arg("--env-file=.env");
                    } else {
                        cmd.env("OLLALINK_API_KEY", "sk_44935c9a9c2186a08697dd56ddc734cb165b94b060aebfd4");
                        cmd.env("PORT", "8787");
                    }
                    cmd.arg("src/server.js");
                    cmd.current_dir(server_root);

                    #[cfg(windows)]
                    {
                        use std::os::windows::process::CommandExt;
                        const CREATE_NO_WINDOW: u32 = 0x08000000;
                        cmd.creation_flags(CREATE_NO_WINDOW);
                    }

                    match cmd.spawn() {
                        Ok(child) => {
                            tracing::info!("Auto-spawned local relay daemon (PID: {}) using {:?} from {}", child.id(), node_bin, server_root.display());
                            if let Ok(mut lock) = RELAY_CHILD.lock() {
                                *lock = Some(child);
                            }

                            // Wait up to 3 seconds for local relay port 8787 to accept TCP connections
                            // Ensures port 8787 is ready BEFORE the Webview probes relay health
                            for _ in 0..30 {
                                std::thread::sleep(std::time::Duration::from_millis(100));
                                if std::net::TcpStream::connect("127.0.0.1:8787").is_ok() {
                                    tracing::info!("Local relay is ready on port 8787");
                                    break;
                                }
                            }

                            return;
                        }
                        Err(e) => {
                            tracing::warn!("Failed to auto-spawn local relay via {:?}: {}", node_bin, e);
                        }
                    }
                }
            }
        }
    }
}

fn cleanup_local_relay() {
    if let Ok(mut lock) = RELAY_CHILD.lock() {
        if let Some(mut child) = lock.take() {
            let _ = child.kill();
        }
    }
}

#[cfg(windows)]
mod single_instance_win {
    #[link(name = "kernel32")]
    extern "system" {
        pub fn CreateMutexW(lpMutexAttributes: *const std::ffi::c_void, bInitialOwner: i32, lpName: *const u16) -> isize;
        pub fn GetLastError() -> u32;
        pub fn CloseHandle(hObject: isize) -> i32;
    }
    #[link(name = "user32")]
    extern "system" {
        pub fn EnumWindows(lpEnumFunc: Option<unsafe extern "system" fn(isize, isize) -> i32>, lParam: isize) -> i32;
        pub fn GetWindowTextW(hWnd: isize, lpString: *mut u16, nMaxCount: i32) -> i32;
        pub fn SetForegroundWindow(hWnd: isize) -> i32;
        pub fn ShowWindow(hWnd: isize, nCmdShow: i32) -> i32;
    }

    pub const ERROR_ALREADY_EXISTS: u32 = 183;
    pub const SW_RESTORE: i32 = 9;

    pub struct MutexGuard(pub isize);
    impl Drop for MutexGuard {
        fn drop(&mut self) {
            if self.0 != 0 {
                unsafe { CloseHandle(self.0); }
            }
        }
    }
}

fn main() {
    // 1. Single Instance Named Mutex Check (Windows)
    // Ensures duplicate launches NEVER initialize a second WebView2 on the same User Data Folder,
    // which completely eliminates WebView2 error 0x800700AA ("The requested resource is in use").
    #[cfg(windows)]
    let _instance_mutex_guard = {
        use single_instance_win::*;
        let mutex_name: Vec<u16> = "Global\\OllalinkTranslateSingleInstanceMutex\0".encode_utf16().collect();
        let handle = unsafe { CreateMutexW(std::ptr::null(), 1, mutex_name.as_ptr()) };
        if handle != 0 && unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
            unsafe {
                unsafe extern "system" fn enum_windows_callback(hwnd: isize, _lparam: isize) -> i32 {
                    let mut title = [0u16; 512];
                    let len = GetWindowTextW(hwnd, title.as_mut_ptr(), 512);
                    if len > 0 {
                        let title_str = String::from_utf16_lossy(&title[..len as usize]);
                        if title_str.contains("Ollalink Translate") {
                            ShowWindow(hwnd, SW_RESTORE);
                            SetForegroundWindow(hwnd);
                            return 0; // stop enumeration
                        }
                    }
                    1 // continue
                }
                EnumWindows(Some(enum_windows_callback), 0);
                CloseHandle(handle);
            }
            // Another instance is already running!
            // Exit immediately BEFORE initializing WebView2 to completely avoid HRESULT 0x800700AA ("The requested resource is in use")
            return;
        }
        MutexGuard(handle)
    };

    // Install default crypto provider for rustls 0.23 (tokio-tungstenite WSS TLS connections)
    let _ = rustls::crypto::ring::default_provider().install_default();

    // Catch any panic and write to persistent log instead of hard crash
    std::panic::set_hook(Box::new(|info| {
        let msg = format!("PANIC: {}", info);
        eprintln!("{}", msg);
        if let Some(mut path) = dirs::data_dir() {
            path.push("com.ollalink.translate");
            let _ = std::fs::create_dir_all(&path);
            path.push("crash.log");
            let _ = std::fs::write(path, &msg);
        }
    }));

    // Structured logging â€” RUST_LOG env overrides the default info level.
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_target(false)
        .with_thread_ids(true)
        .compact()
        .init();

    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.show();
                let _ = w.unminimize();
                let _ = w.set_focus();
            }
        }))
        .plugin(tauri_plugin_shell::init())
        .setup(|app| {
            try_spawn_local_relay();
            let handle = app.handle().clone();
            let state = AppState::new(handle);
            app.manage(state);
            Ok(())
        })
        .on_window_event(|_window, event| {
            if let tauri::WindowEvent::Destroyed = event {
                cleanup_local_relay();
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::mint_session,
            commands::list_audio_devices,
            commands::start_call,
            commands::end_call,
            commands::call_status,
            commands::set_input_volume,
            commands::set_mic_muted,
            commands::set_speaker_muted,
            commands::swap_input_device,
            commands::swap_output_device,
            commands::load_prefs,
            commands::save_prefs,
            commands::set_captions,
            commands::change_languages,
            commands::update_voice_settings,
            commands::check_relay_health,
            commands::play_audio_chunk,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

// ---------------------------------------------------------------------------
// Tauri commands â€” the UI â†” Rust bridge. All async, all return Result<_, String>.
// ---------------------------------------------------------------------------

pub mod commands {
    use super::state::{AppState, CallArgs, CallState, SessionCredentials};
    use serde::Serialize;
    use tauri::State;

    /// Mint a session token via the relay server.
    /// Returns the WsUrl and token the app will use when starting a call.
    #[tauri::command]
    pub async fn mint_session(
        relay_url: String,
        user_id: String,
        source_lang: String,
        target_lang: String,
        voice: Option<String>,
        tone: Option<String>,
    ) -> Result<SessionCredentials, String> {
        crate::ws::mint_session_via_relay(
            &relay_url,
            &user_id,
            &source_lang,
            &target_lang,
            voice.as_deref(),
            tone.as_deref(),
        )
        .await
        .map_err(|e| e.to_string())
    }

    /// Start a call: join a room, open the Ollalink upstream, start audio.
    #[tauri::command]
    pub async fn start_call(
        state: State<'_, AppState>,
        args: CallArgs,
    ) -> Result<serde_json::Value, String> {
        state.start_call(args).await.map_err(|e| e.to_string())
    }

    /// End the current call.
    #[tauri::command]
    pub async fn end_call(state: State<'_, AppState>) -> Result<(), String> {
        state.end_call().await.map_err(|e| e.to_string())
    }

    /// Snapshot of current call state for the UI.
    #[tauri::command]
    pub async fn call_status(state: State<'_, AppState>) -> Result<CallState, String> {
        Ok(state.status().await)
    }

    /// Enumerate input/output audio devices for the picklist.
    #[tauri::command]
    pub async fn list_audio_devices() -> Result<AudioDeviceList, String> {
        crate::audio::list_devices().map_err(|e| e.to_string())
    }

        /// Mute or unmute the microphone mid-call.
    #[tauri::command]
    pub async fn set_mic_muted(state: State<'_, AppState>, muted: bool) -> Result<(), String> {
        state.set_mic_muted(muted);
        Ok(())
    }

    /// Mute or unmute speaker output mid-call.
    #[tauri::command]
    pub async fn set_speaker_muted(state: State<'_, AppState>, muted: bool) -> Result<(), String> {
        state.set_speaker_muted(muted);
        Ok(())
    }

    /// Live input gain (0.0–2.0).
    #[tauri::command]
    pub async fn set_input_volume(state: State<'_, AppState>, volume: f32) -> Result<(), String> {
        state.set_input_volume(volume);
        Ok(())
    }

    /// Hot-swap microphone mid-call. Pass null to use default.
    #[tauri::command]
    pub async fn swap_input_device(
        state: State<'_, AppState>,
        name: Option<String>,
    ) -> Result<(), String> {
        state.swap_input_device(name).await.map_err(|e| e.to_string())
    }

    /// Hot-swap speaker mid-call. Pass null to use default.
    #[tauri::command]
    pub async fn swap_output_device(
        state: State<'_, AppState>,
        name: Option<String>,
    ) -> Result<(), String> {
        state.swap_output_device(name).await.map_err(|e| e.to_string())
    }

    /// Load persisted user prefs (display name, langs, devices, relay URL).
    #[tauri::command]
    pub async fn load_prefs() -> Result<crate::prefs::UserPrefs, String> {
        Ok(crate::prefs::load())
    }

    /// Save user prefs to disk.
    #[tauri::command]
    pub async fn save_prefs(prefs: crate::prefs::UserPrefs) -> Result<(), String> {
        crate::prefs::save(&prefs).map_err(|e| e.to_string())
    }

    /// Toggle live captions (mid-call).
    #[tauri::command]
    pub async fn set_captions(state: State<'_, AppState>, on: bool) -> Result<(), String> {
        state.set_captions(on).await.map_err(|e| e.to_string())
    }

    /// Change languages mid-call.
    #[tauri::command]
    pub async fn change_languages(
        state: State<'_, AppState>,
        source_lang: Option<String>,
        target_lang: Option<String>,
    ) -> Result<(), String> {
        state.change_languages(source_lang, target_lang).await.map_err(|e| e.to_string())
    }

    /// Update voice persona and tone mid-call.
    #[tauri::command]
    pub async fn update_voice_settings(
        state: State<'_, AppState>,
        voice: Option<String>,
        tone: Option<String>,
    ) -> Result<(), String> {
        state.update_voice_settings(voice, tone).await.map_err(|e| e.to_string())
    }

    /// Probe relay health bypassing webview CORS.
    #[tauri::command]
        pub async fn check_relay_health(relay_url: String) -> Result<bool, String> {
        let base = relay_url.trim().trim_end_matches('/');
        let url = format!("{}/api/health", base);
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_millis(1500))
            .build()
            .map_err(|e| e.to_string())?;
        match client.get(&url).send().await {
            Ok(res) => Ok(res.status().is_success()),
            Err(_) => Ok(false),
        }
    }

    #[tauri::command]
    pub async fn play_audio_chunk(
        state: State<'_, AppState>,
        pcm_base64: String,
        sample_rate: u32,
        is_last: bool,
    ) -> Result<(), String> {
        use base64::Engine;
        if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(&pcm_base64) {
            state.push_inbound_audio(bytes, sample_rate, is_last).await;
        }
        Ok(())
    }

    #[derive(Debug, Serialize)]
    pub struct AudioDeviceList {
        pub inputs: Vec<String>,
        pub outputs: Vec<String>,
        pub default_input: Option<String>,
        pub default_output: Option<String>,
    }

    // Re-export so the commands module exposes SessionCredentials to TS via specta/serde.
    pub use crate::state::SessionCredentials as _SC;
}
