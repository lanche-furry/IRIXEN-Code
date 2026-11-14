//! Small shared helpers.

use std::sync::{Mutex, MutexGuard};

/// Lock a mutex, recovering from a poisoned lock instead of panicking.
/// A panic inside a command handler must not brick the whole app state.
pub fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Turn a Windows verbatim path (`\\?\E:\dir`, `\\?\UNC\srv\share`) into the
/// ordinary form.
///
/// `canonicalize()` always returns verbatim paths on Windows. They are valid,
/// but they do not accept `/` as a separator, they are rejected by a number of
/// Win32 APIs and by `cmd`, and they read as garbage in the UI. Stripping the
/// prefix keeps the characters — including Chinese ones — untouched.
pub fn plain_path(path: &str) -> String {
    if let Some(rest) = path.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = path.strip_prefix(r"\\?\") {
        rest.to_string()
    } else {
        path.to_string()
    }
}

/// Decode a child process' stdout/stderr.
///
/// Every process we spawn is started on a UTF-8 console (`chcp 65001`) with
/// `PYTHONUTF8=1` / `PYTHONIOENCODING=utf-8`, so UTF-8 is the normal case.
/// Legacy tools that still write the OEM code page — a JDK whose
/// `file.encoding` is GBK, an old CLI — would otherwise turn every Chinese
/// character into `?`, so a failed UTF-8 decode is retried as GBK.
///
/// The caller must pass a complete buffer (a whole line or a whole stream):
/// half a UTF-8 character would look like invalid UTF-8 and be decoded as GBK,
/// which is where mojibake comes from.
pub fn decode_output(bytes: &[u8]) -> String {
    if let Ok(text) = std::str::from_utf8(bytes) {
        return text.to_string();
    }
    let (text, _, _) = encoding_rs::GBK.decode(bytes);
    if text.contains('\u{FFFD}') {
        // Neither UTF-8 nor GBK: keep the printable bytes instead of dropping
        // the line entirely.
        return String::from_utf8_lossy(bytes).to_string();
    }
    text.to_string()
}

/// Make a child command speak UTF-8 on Windows.
///
/// `cmd` itself and most CLI tools write the console code page (GBK on a
/// Chinese install). Chinese paths and messages then turned into `?` in the
/// terminal. Switching the console page to 65001 and telling Python to use
/// UTF-8 fixes the source instead of guessing on the receiving end.
pub fn utf8_env(cmd: &mut tokio::process::Command) {
    cmd.env("PYTHONUTF8", "1")
        .env("PYTHONIOENCODING", "utf-8")
        .env("PYTHONLEGACYWINDOWSSTDIO", "0");
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Hard-cap a string for tool output so one huge read can't blow the context.
pub fn cap_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let head: String = s.chars().take(max).collect();
        let total = s.chars().count();
        format!("{head}\n… [truncated, {total} chars total]")
    }
}

/// Compact, single-line preview of text for logs and UI badges.
pub fn one_line(s: &str, max: usize) -> String {
    let line: String = s
        .chars()
        .take(max)
        .map(|c| if c == '\n' || c == '\r' || c == '\t' { ' ' } else { c })
        .collect();
    line.trim().to_string()
}

pub fn short_id() -> String {
    let n = now_ms();
    format!("{:x}", n.wrapping_mul(0x9E3779B97F4A7C15) & 0xffff_ffff_ffff)
}

/// Hand a URL to the OS default browser.
///
/// Deliberately restricted to http(s) and mailto: an extension calling
/// `env.openExternal` must not be a way to launch `cmd.exe` with arguments.
pub fn open_external(url: &str) -> Result<(), String> {
    let trimmed = url.trim();
    let lower = trimmed.to_ascii_lowercase();
    if !(lower.starts_with("http://") || lower.starts_with("https://") || lower.starts_with("mailto:")) {
        return Err("只允许打开 http/https/mailto 链接".into());
    }
    if trimmed.contains('\n') || trimmed.contains('\r') {
        return Err("链接里不能包含换行".into());
    }

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // `cmd /C start "" <url>` is the portable incantation; the empty title
        // keeps `start` from treating a quoted URL as the window title.
        std::process::Command::new("cmd")
            .args(["/C", "start", "", trimmed])
            .creation_flags(0x0800_0000)
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("打开链接失败：{e}"))
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(trimmed)
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("打开链接失败：{e}"))
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        std::process::Command::new("xdg-open")
            .arg(trimmed)
            .spawn()
            .map(|_| ())
            .map_err(|e| format!("打开链接失败：{e}"))
    }
}

