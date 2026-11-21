//! IRIXEN Code — application entry, module wiring and window lifecycle.

pub mod agent;
pub mod bridge;
pub mod commands;
pub mod config;
pub mod extensions;
pub mod exthost;
pub mod git;
pub mod mcp;
pub mod net;
pub mod runner;
pub mod single_instance;
pub mod skills;
pub mod state;
pub mod terminal;
pub mod tray;
pub mod update;
pub mod util;
pub mod websession;
pub mod wvproto;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tauri::Manager;

use agent::AgentEngine;
use bridge::Bridge;
use config::AppConfig;
use mcp::McpHost;
use skills::SkillRegistry;
use state::AppState;

/// Set the first time the UI reports in; only used to log the boot timing.
static MAIN_LOADED: AtomicBool = AtomicBool::new(false);
/// Process start, used only for the boot timing line in the log.
static BOOT_AT: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();

/// Make sure the frame is on screen and focused. There is no splash window and
/// no reveal gate: the window is created visible (tauri.conf.json) so the app
/// comes up straight away, with the page painting the app's own background
/// colour until the first real frame lands.
pub fn reveal_main(app: &tauri::AppHandle) {
    if let Some(main) = app.get_webview_window("main") {
        let _ = main.unminimize();
        let _ = main.show();
        let _ = main.set_focus();
    }
}

/// Called from the UI once it has mounted. The window is already visible; this
/// only takes focus back (another window may have grabbed it during a slow
/// `vite` cold start) and records the timing in the log.
pub fn mark_main_loaded(app: &tauri::AppHandle) {
    if !MAIN_LOADED.swap(true, Ordering::SeqCst) {
        let ms = BOOT_AT.get().map(|t| t.elapsed().as_millis()).unwrap_or(0);
        eprintln!("[irixen] ui mounted after {ms}ms");
    }
    reveal_main(app);
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // A second copy would die inside `setup` with a bare WebView2 error (the
    // user-data folder is locked by the first one) — no window, no message.
    // Hand the launch to the running instance instead; see single_instance.rs.
    if !single_instance::claim() {
        return;
    }
    BOOT_AT.get_or_init(std::time::Instant::now);
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        // Extension webview panels are served from their own origin instead of
        // an `<iframe srcdoc>`; see `wvproto` for why that matters.
        .register_uri_scheme_protocol("irixen-wv", |_ctx, request| {
            crate::wvproto::handle(&request)
        })
        .setup(|app| {
            let handle = app.handle().clone();
            websession::init(handle.clone());
            let cfg = AppConfig::load();
            // Source metadata is cheap in-memory state: keep it available so
            // the Web2API panel can list sources without starting anything.
            websession::sync_sources(&cfg.bridge);
            // Hidden WebView2 windows are by far the most expensive thing this
            // app can start — one renderer process group per source. They only
            // come up when the user turned the bridge on in Settings → Advanced,
            // confirmed full access in the panel *and* enabled "start on launch".
            // A manual start (`web2api_start` / `ensure`) brings up the watcher
            // and the sessions on demand instead.
            if cfg.bridge.enabled && cfg.bridge.full_access && cfg.bridge.auto_start {
                websession::start_watcher();
                let boot_bridge = cfg.bridge.clone();
                tauri::async_runtime::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_millis(900)).await;
                    websession::ensure_sessions(&boot_bridge, None).await;
                });
            }
            // load() merges newly added presets (the built-in Web2API provider
            // and NVIDIA NIM) in memory only. Persist once so the
            // auto-configured providers also exist in config.json and survive
            // the next process instead of being re-added on every boot.
            let _ = cfg.save();
            // Skills and extensions live next to the executable by default
            // (Settings → Advanced can point them elsewhere). Create the
            // folders and pull in anything a previous build left in the profile
            // folder *before* the registry scans them.
            crate::config::ensure_addons();
            let config = Arc::new(Mutex::new(cfg.clone()));
            let skills = Arc::new(SkillRegistry::load(
                &cfg.skills.enabled,
                cfg.skills.auto_match,
            ));
            let mcp = Arc::new(McpHost::new());
            let engine = AgentEngine::new(
                handle.clone(),
                config.clone(),
                skills.clone(),
                mcp.clone(),
            );

            app.manage(AppState {
                config: config.clone(),
                engine: engine.clone(),
                skills: skills.clone(),
                mcp: mcp.clone(),
            });
            app.manage(engine);
            app.manage(skills);

            // File runner: one child process per run, output streamed to the
            // renderer's panel.
            app.manage(std::sync::Arc::new(runner::RunnerService::new()));

            // Integrated terminal: one long-lived PTY session per tab, so the
            // shell keeps its prompt, history and working directory.
            app.manage(std::sync::Arc::new(terminal::TermService::new()));

            // Node extension host — one child process per extension.
            let exthost = Arc::new(exthost::ExtHost::new(handle.clone(), config.clone()));
            app.manage(exthost.clone());

            // Web2API bridge — a local OpenAI/Anthropic-compatible endpoint.
            // Nothing is bound to a port unless the master switch in
            // Settings → Advanced is on *and* full access was confirmed in the
            // panel; an install that never opts in leaves no listener behind,
            // and a switch that was cycled off and on again asks again instead
            // of resuming on the next boot.
            let bridge = Arc::new(Bridge::new(config.clone()));
            app.manage(bridge.clone());
            if cfg.bridge.enabled && cfg.bridge.full_access && cfg.bridge.auto_start {
                let bridge = bridge.clone();
                tauri::async_runtime::spawn(async move {
                    if let Err(e) = bridge.start().await {
                        eprintln!("[irixen] web2api auto-start failed: {e}");
                        bridge.fail(e);
                    }
                });
            }

            // No splash window, no timed intro, no reveal gate: the frame is
            // created visible by tauri.conf.json, so the window is there as soon
            // as the process starts. The UI reports in later (`main_loaded`) and
            // only grabs focus / logs the timing.
            //
            // Extensions that ask for `onStartupFinished` boot slightly after the
            // first paint, otherwise a cold `npm`-sized extension can compete with
            // the window for the disk and make startup feel broken.
            {
                let exthost = exthost.clone();
                let delay = cfg.extensions.autostart_delay_ms;
                tauri::async_runtime::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
                    exthost.autostart();
                });
            }

            // Auto-start MCP servers flagged for it.
            let mcp_boot = mcp.clone();
            let servers = cfg.mcp_servers.clone();
            tauri::async_runtime::spawn(async move {
                for s in servers.iter().filter(|s| s.enabled && s.auto_start) {
                    if let Err(e) = mcp_boot.start(s).await {
                        eprintln!("[irixen] mcp '{}' failed to start: {e}", s.id);
                    }
                }
            });

            // Tray icon: the way back to a window that was closed into the tray,
            // and the only place a real quit is reachable when
            // `close_to_tray` is on. A failure here is not fatal — the close
            // handler checks `tray::ready()` and keeps the old behaviour.
            if let Err(e) = tray::install(&handle, &cfg.language) {
                eprintln!("[irixen] tray icon unavailable: {e}");
            }

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_config,
            commands::save_config,
            commands::config_reset,
            commands::app_info,
            commands::open_addons_folder,
            commands::app_check_update,
            commands::app_download_update,
            commands::app_apply_update,
            commands::set_workspace,
            commands::provider_upsert,
            commands::provider_delete,
            commands::provider_test,
            commands::provider_probe,
            commands::models_list,
            commands::agent_sessions,
            commands::agent_new_session,
            commands::agent_set_active,
            commands::agent_active,
            commands::agent_messages,
            commands::agent_send,
            commands::agent_stop,
            commands::agent_clear,
            commands::agent_delete,
            commands::agent_undo_turn,
            commands::agent_set_model,
            commands::set_default_effort,
            commands::set_agent_access,
            commands::agent_approve,
            commands::agent_busy,
            commands::agent_usage,
            commands::agent_reset_usage,
            commands::fs_list,
            commands::fs_read,
            commands::fs_write,
            commands::fs_create_file,
            commands::fs_create_dir,
            commands::fs_delete,
            commands::fs_rename,
            commands::fs_search,
            commands::shell_run,
            commands::language_set,
            commands::runner_options,
            commands::runner_detect,
            commands::runner_save,
            commands::runner_reset,
            commands::file_run,
            commands::file_run_stop,
            commands::shell_stream,
            commands::term_shells,
            commands::term_open,
            commands::term_write,
            commands::term_resize,
            commands::term_close,
            commands::term_attach,
            commands::term_list,
            commands::git_is_repo,
            commands::git_status,
            commands::git_diff,
            commands::git_stage,
            commands::git_unstage,
            commands::git_commit,
            commands::git_branch,
            commands::git_branches,
            commands::git_checkout,
            commands::git_log,
            commands::git_init,
            commands::git_push,
            commands::git_pull,
            commands::workspace_stats,
            commands::mcp_servers,
            commands::mcp_upsert,
            commands::mcp_delete,
            commands::mcp_start,
            commands::mcp_stop,
            commands::mcp_tools,
            commands::skills_list,
            commands::skills_install,
            commands::skills_uninstall,
            commands::skills_toggle,
            commands::skills_read,
            commands::main_loaded,
            commands::window_control,
            commands::web2api_status,
            commands::web2api_set_full_access,
            commands::web2api_start,
            commands::web2api_stop,
            commands::web2api_log,
            commands::web2api_open_session,
            commands::web2api_hide_session,
            commands::web2api_session_status,
            commands::web2api_probe,
            commands::web2api_ensure,
            commands::web2api_test,
            commands::extensions_list,
            commands::extensions_install,
            commands::extensions_uninstall,
            commands::extensions_toggle,
            commands::extensions_search,
            commands::extensions_detail,
            commands::extensions_inspect,
            commands::extensions_check_updates,
            commands::extensions_theme,
            commands::extensions_snippets,
            commands::exthost_node,
            commands::exthost_install_node,
            commands::exthost_status,
            commands::exthost_start,
            commands::exthost_stop,
            commands::exthost_stop_all,
            commands::exthost_restart_all,
            commands::exthost_restart,
            commands::exthost_logs,
            commands::exthost_clear_logs,
            commands::exthost_commands,
            commands::exthost_statusbar,
            commands::exthost_run,
            commands::exthost_reply,
            commands::exthost_doc,
            commands::ext_language,
            commands::ext_providers,
            commands::webview_post,
            commands::webview_close,
            commands::webview_publish,
            commands::webview_forget,
            commands::webview_base,
            commands::webview_list,
            commands::preview_probe,
            commands::exttree_views,
            commands::exttree_children,
            commands::exttree_command,
            commands::exttree_refresh,
            commands::open_external,
        ])
        // A closed vendor window must NOT be destroyed: its cookie jar is the
        // whole point of Web2API. Hide it instead so the session stays logged
        // in and the next request needs no manual "打开官网" click.
        //
        // The main window is the other close that means "hide": with
        // `close_to_tray` on (the default), the titlebar's X parks the app in
        // the tray instead of killing an agent turn or a running build. The
        // setting is read per event, so flipping it applies immediately, and
        // the tray must actually exist — otherwise the window would vanish
        // with nothing to bring it back.
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if window.label().starts_with("web2api-") {
                    api.prevent_close();
                    let _ = window.hide();
                    return;
                }
                if window.label() == "main" {
                    let hide = window
                        .app_handle()
                        .try_state::<AppState>()
                        .map(|st| crate::util::lock(&st.config).close_to_tray)
                        .unwrap_or(false);
                    if hide && tray::ready() {
                        api.prevent_close();
                        let _ = window.hide();
                    } else {
                        // "Quit on close" has to mean the process, not just this
                        // window: a bridge session leaves hidden WebView
                        // windows behind, and those would keep the app alive
                        // with nothing on screen to quit it from.
                        window.app_handle().exit(0);
                    }
                }
            }
        })
        .build(tauri::generate_context!())
        .expect("error while building IRIXEN Code")
        .run(|app, event| {
            if let tauri::RunEvent::ExitRequested { .. } = event {
                if let Some(host) = app.try_state::<Arc<exthost::ExtHost>>() {
                    host.stop_all();
                }
            }
        });
}
