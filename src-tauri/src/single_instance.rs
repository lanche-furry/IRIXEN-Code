//! One process per Windows session.
//!
//! WebView2 keeps its user-data folder (`%LOCALAPPDATA%\com.irixen.code`) locked
//! for the lifetime of a process. A second copy therefore fails inside
//! `tauri::Builder::setup` with
//!
//! ```text
//! failed to create webview: WebView2 error: WindowsError(HRESULT(0x8007139F))
//! 组或资源的状态不是执行请求操作的正确状态。
//! ```
//!
//! which reads as "the app is broken" — it panics before a window exists, so
//! there is nothing on screen to explain it. That is exactly what happens when
//! someone installs a new build while an older copy (or a probe instance from
//! `scripts/dev`) is still open, then launches the installed shortcut.
//!
//! The fix is the usual desktop one: a named mutex for the session. If it is
//! already held, raise the running window instead of starting a second one.
//!
//! Raw `kernel32` declarations instead of a new crate: this is three calls and
//! keeps the dependency tree (and the offline build) untouched.

#[cfg(windows)]
mod imp {
    use std::ffi::c_void;

    #[link(name = "kernel32")]
    extern "system" {
        fn CreateMutexW(attrs: *mut c_void, owner: i32, name: *const u16) -> *mut c_void;
        fn GetLastError() -> u32;
        fn FindWindowW(class: *const u16, title: *const u16) -> *mut c_void;
        fn ShowWindow(hwnd: *mut c_void, cmd: i32) -> i32;
        fn SetForegroundWindow(hwnd: *mut c_void) -> i32;
    }

    const ERROR_ALREADY_EXISTS: u32 = 183;
    const SW_RESTORE: i32 = 9;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// `true` when this process owns the session mutex. `false` means another
    /// IRIXEN Code is already running.
    ///
    /// The handle is deliberately leaked: it must live as long as the process,
    /// and the OS releases the mutex on exit (including on a crash, so a killed
    /// instance can never leave a stale lock behind).
    pub fn acquire() -> bool {
        let name = wide("Local\\IRIXEN.Code.SingleInstance");
        let handle = unsafe { CreateMutexW(std::ptr::null_mut(), 0, name.as_ptr()) };
        if handle.is_null() {
            // Cannot create a mutex at all: do not block the user over it.
            return true;
        }
        // CreateMutexW reports "already there" through the last-error slot, not
        // through the return value, so read it before anything else runs.
        let last = unsafe { GetLastError() };
        last != ERROR_ALREADY_EXISTS
    }

    /// Bring the window that already exists to the front. Best effort: the
    /// other instance may still be painting its first frame.
    pub fn focus_existing() {
        let title = wide("IRIXEN Code");
        unsafe {
            let hwnd = FindWindowW(std::ptr::null(), title.as_ptr());
            if !hwnd.is_null() {
                ShowWindow(hwnd, SW_RESTORE);
                SetForegroundWindow(hwnd);
            }
        }
    }
}

/// Hand the launch to the instance that is already running, if any.
///
/// Returns `true` when this process should continue booting. On a second
/// launch it focuses the existing window and returns `false`; the caller exits
/// cleanly, before any WebView2 or config work happens.
pub fn claim() -> bool {
    #[cfg(windows)]
    {
        if imp::acquire() {
            return true;
        }
        imp::focus_existing();
        false
    }
    #[cfg(not(windows))]
    {
        true
    }
}
