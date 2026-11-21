//! The tray icon: the one piece of this app that outlives its window.
//!
//! Closing the window can mean two different things. For a run that is halfway
//! through an agent turn, a `cargo build` in the integrated terminal, or a
//! Node extension host that took ten seconds to boot, quitting on close throws
//! all of that away. So the close button hides the window by default, and the
//! tray icon is how it comes back — plus the one place a real quit lives.
//!
//! `AppConfig::close_to_tray` turns that off for people who expect a close
//! button to close. That switch is only honest if a quit is always reachable
//! without it, which is what the menu item below is for.

use std::sync::atomic::{AtomicBool, Ordering};

use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};

/// Whether a tray icon actually exists in this session.
///
/// Hiding the window is only safe while it does: on a Linux desktop without a
/// StatusNotifier host, `build()` can fail outright, and a window that hides
/// with no icon to bring it back is a process the user cannot reach. The close
/// handler reads this and falls back to a normal close.
static TRAY_READY: AtomicBool = AtomicBool::new(false);

pub fn ready() -> bool {
    TRAY_READY.load(Ordering::Relaxed)
}

struct Labels {
    show: &'static str,
    quit: &'static str,
    tip: &'static str,
}

/// The menu is built once at startup, so its labels follow the language the
/// config held then; a language change applies on the next launch. Relabelling
/// a native menu in place would mean keeping the item handles alive for the
/// whole process, which is more moving parts than a two-item menu is worth.
fn labels(language: &str) -> Labels {
    match language {
        "zh-TW" => Labels {
            show: "顯示主視窗",
            quit: "結束",
            tip: "IRIXEN Code — 關閉視窗後仍在執行",
        },
        "ja-JP" => Labels {
            show: "ウィンドウを表示",
            quit: "終了",
            tip: "IRIXEN Code — 実行中",
        },
        "ko-KR" => Labels {
            show: "창 표시",
            quit: "종료",
            tip: "IRIXEN Code — 실행 중",
        },
        "ru-RU" => Labels {
            show: "Показать окно",
            quit: "Выход",
            tip: "IRIXEN Code — работает",
        },
        "es-ES" => Labels {
            show: "Mostrar ventana",
            quit: "Salir",
            tip: "IRIXEN Code — en ejecución",
        },
        "fr-FR" => Labels {
            show: "Afficher la fenêtre",
            quit: "Quitter",
            tip: "IRIXEN Code — en cours d'exécution",
        },
        "en-US" => Labels {
            show: "Show window",
            quit: "Quit",
            tip: "IRIXEN Code — still running",
        },
        // zh-CN is the default language, so it is also the fallback.
        _ => Labels {
            show: "显示主窗口",
            quit: "退出",
            tip: "IRIXEN Code — 关闭窗口后仍在这里",
        },
    }
}

pub fn install(app: &tauri::AppHandle, language: &str) -> tauri::Result<()> {
    let l = labels(language);
    let show = MenuItem::with_id(app, "show", l.show, true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", l.quit, true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &quit])?;

    let mut builder = TrayIconBuilder::with_id("main")
        .tooltip(l.tip)
        .menu(&menu)
        // Left click is "bring it back"; the menu lives on the right button.
        // Both platforms that ship a build here follow that convention, and a
        // menu that opens on every left click makes the icon feel broken.
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().0.as_str() {
            "show" => crate::reveal_main(app),
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                crate::reveal_main(tray.app_handle());
            }
        });

    // Same artwork as the taskbar icon; `default_window_icon` is the image the
    // bundle already carries, so this adds no bytes to the installer.
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }

    builder.build(app)?;
    TRAY_READY.store(true, Ordering::Relaxed);
    Ok(())
}
