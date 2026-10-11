//! Shows a file selected in an Explorer window without taking over the
//! screen (Windows only).
//!
//! `SHOpenFolderAndSelectItems` opens the window, as `explorer
//! /select,<path>` does not select the file in a folder never opened in
//! Explorer before. Neither can open a window minimized or behind the
//! others. So while it opens, no window may take the keyboard by itself
//! (`LockSetForegroundWindow`), and the new window is minimized as soon as
//! it appears. The user's own clicks and keys lift that lock, and Explorer
//! may then take the keyboard into its minimized window; it is given back
//! where Windows allows it.

/// Opens the folder of `path` (a Windows path) with the file selected, then
/// minimizes the window. Returns whether the folder was opened; it waits up
/// to about 15 seconds for the window, so the hook starts it in the
/// background.
#[cfg(windows)]
pub fn show_selected(path: &str) -> bool {
    use std::time::Duration;

    let front = win::foreground();
    let locked = win::lock_foreground(true);
    let (opened, window) = open_minimized(path, front, locked);
    if locked {
        win::lock_foreground(false);
    }
    // Explorer may still try to come to the front once it may again.
    if let Some(window) = window {
        keep_minimized(window, front, false, Duration::from_millis(1500));
    }
    opened
}

/// Whether the call opened the folder, and the window it opened in.
#[cfg(windows)]
fn open_minimized(path: &str, front: isize, locked: bool) -> (bool, Option<isize>) {
    use std::time::{Duration, Instant};

    let folder = path
        .rsplit_once('\\')
        .map(|(dir, _)| dir.rsplit('\\').next().unwrap_or(dir))
        .unwrap_or_default()
        .to_string();
    let before = win::explorer_windows();
    // The call often returns only once the window is shown, up to 5 seconds
    // later, so the window is watched for while it runs; it may also return
    // before, so the window is looked for until the time is up either way.
    let target = path.to_string();
    let opener = std::thread::spawn(move || win::open_selected(&target));
    let started = Instant::now();
    let mut window = None;
    while window.is_none() && started.elapsed() < Duration::from_secs(10) {
        // A new window (its title may not name the folder yet), or an open
        // one the folder went to.
        let fg = win::foreground();
        window = win::explorer_windows()
            .into_iter()
            .find(|w| !before.contains(w) || (*w == fg && win::title(*w).contains(&folder)));
        if window.is_none() {
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    if let Some(window) = window {
        keep_minimized(window, front, locked, Duration::from_secs(3));
    }
    // Watched until the call is done, and a moment after.
    while !opener.is_finished() {
        if let Some(window) = window {
            keep_minimized(window, front, locked, Duration::from_millis(100));
        } else {
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    let opened = opener.join().unwrap_or(false);
    if let Some(window) = window {
        keep_minimized(window, front, locked, Duration::from_millis(500));
    }
    (opened, window)
}

/// Explorer may show the window again or take the keyboard while it
/// finishes opening, so it is watched for `time` and put back each time.
#[cfg(windows)]
fn keep_minimized(window: isize, front: isize, locked: bool, time: std::time::Duration) {
    use std::time::{Duration, Instant};
    let started = Instant::now();
    while started.elapsed() < time {
        if !win::is_minimized(window) {
            win::minimize_quietly(window);
        }
        if win::foreground() == window && front != 0 && front != window {
            // The lock stops this call too, so it is lifted for it.
            if locked {
                win::lock_foreground(false);
            }
            win::bring_to_front(front);
            if locked {
                win::lock_foreground(true);
            }
            // Windows may refuse that to a process in the background; a
            // window minimized the plain way may hand the keyboard on itself.
            if win::foreground() == window {
                win::minimize_passing_on(window);
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(not(windows))]
pub fn show_selected(_path: &str) -> bool {
    false
}

#[cfg(windows)]
mod win {
    use std::ffi::c_void;
    use std::os::windows::ffi::OsStrExt;

    type Hwnd = isize;

    #[link(name = "shell32")]
    unsafe extern "system" {
        fn ILCreateFromPathW(path: *const u16) -> *mut c_void;
        fn ILFree(pidl: *mut c_void);
        fn SHOpenFolderAndSelectItems(
            folder: *const c_void,
            count: u32,
            items: *const *const c_void,
            flags: u32,
        ) -> i32;
    }
    #[link(name = "ole32")]
    unsafe extern "system" {
        fn CoInitializeEx(reserved: *mut c_void, coinit: u32) -> i32;
        fn CoUninitialize();
    }
    #[link(name = "user32")]
    unsafe extern "system" {
        fn EnumWindows(callback: extern "system" fn(Hwnd, isize) -> i32, param: isize) -> i32;
        fn GetClassNameW(window: Hwnd, name: *mut u16, len: i32) -> i32;
        fn GetWindowTextW(window: Hwnd, text: *mut u16, len: i32) -> i32;
        fn IsWindowVisible(window: Hwnd) -> i32;
        fn IsWindow(window: Hwnd) -> i32;
        fn IsIconic(window: Hwnd) -> i32;
        fn GetForegroundWindow() -> Hwnd;
        fn SetForegroundWindow(window: Hwnd) -> i32;
        fn ShowWindow(window: Hwnd, show: i32) -> i32;
        fn LockSetForegroundWindow(code: u32) -> i32;
        fn GetWindowThreadProcessId(window: Hwnd, process: *mut u32) -> u32;
        fn AttachThreadInput(from: u32, to: u32, attach: i32) -> i32;
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCurrentThreadId() -> u32;
    }
    const COINIT_APARTMENTTHREADED: u32 = 2;
    const SW_SHOWMINNOACTIVE: i32 = 7;

    pub fn open_selected(path: &str) -> bool {
        let wide: Vec<u16> = std::ffi::OsStr::new(path)
            .encode_wide()
            .chain(Some(0))
            .collect();
        // SAFETY: `wide` ends with 0 and outlives the calls; the item list
        // is freed once, after its last use; COM is left only if entered.
        unsafe {
            let entered = CoInitializeEx(std::ptr::null_mut(), COINIT_APARTMENTTHREADED) >= 0;
            let pidl = ILCreateFromPathW(wide.as_ptr());
            let ok =
                !pidl.is_null() && SHOpenFolderAndSelectItems(pidl, 0, std::ptr::null(), 0) >= 0;
            if !pidl.is_null() {
                ILFree(pidl);
            }
            if entered {
                CoUninitialize();
            }
            ok
        }
    }

    fn text(read: impl Fn(*mut u16, i32) -> i32) -> String {
        let mut buf = [0u16; 512];
        let len = read(buf.as_mut_ptr(), buf.len() as i32).max(0) as usize;
        String::from_utf16_lossy(&buf[..len])
    }

    pub fn title(window: Hwnd) -> String {
        // SAFETY: the buffer is as long as the length passed.
        text(|buf, len| unsafe { GetWindowTextW(window, buf, len) })
    }

    extern "system" fn collect(window: Hwnd, param: isize) -> i32 {
        // SAFETY: `param` is the Vec passed by `explorer_windows`, which
        // outlives EnumWindows; the buffer is as long as the length passed.
        unsafe {
            let found = &mut *(param as *mut Vec<Hwnd>);
            let class = text(|buf, len| GetClassNameW(window, buf, len));
            if class == "CabinetWClass" && IsWindowVisible(window) != 0 {
                found.push(window);
            }
        }
        1
    }

    /// The visible Explorer folder windows.
    pub fn explorer_windows() -> Vec<Hwnd> {
        let mut found: Vec<Hwnd> = Vec::new();
        // SAFETY: `collect` only reads `found` through the pointer while
        // EnumWindows runs.
        unsafe { EnumWindows(collect, &mut found as *mut Vec<Hwnd> as isize) };
        found
    }

    pub fn foreground() -> Hwnd {
        // SAFETY: no arguments.
        unsafe { GetForegroundWindow() }
    }

    /// Stops (`true`) or lets again (`false`) windows take the keyboard by
    /// themselves. Returns whether Windows took the call.
    pub fn lock_foreground(lock: bool) -> bool {
        const LSFW_LOCK: u32 = 1;
        const LSFW_UNLOCK: u32 = 2;
        // SAFETY: no pointers.
        unsafe { LockSetForegroundWindow(if lock { LSFW_LOCK } else { LSFW_UNLOCK }) != 0 }
    }

    pub fn is_minimized(window: Hwnd) -> bool {
        // SAFETY: a window handle; a stale one reads as not minimized.
        unsafe { IsIconic(window) != 0 }
    }

    /// Minimizes `window` and gives the keyboard to the next window.
    pub fn minimize_passing_on(window: Hwnd) {
        const SW_MINIMIZE: i32 = 6;
        // SAFETY: a window handle; a stale one is ignored by Windows.
        unsafe { ShowWindow(window, SW_MINIMIZE) };
    }

    pub fn minimize_quietly(window: Hwnd) {
        // SAFETY: a window handle; a stale one is ignored by Windows.
        unsafe { ShowWindow(window, SW_SHOWMINNOACTIVE) };
    }

    /// Tries to give `window` the keyboard back. Windows often refuses this
    /// to a process in the background; sharing the input of the window in
    /// front, as this thread does for the call, lets it through at times.
    pub fn bring_to_front(window: Hwnd) {
        // SAFETY: window handles and thread ids; a stale one makes a call
        // fail, which leaves things as they are. Joined input is undone.
        unsafe {
            if IsWindow(window) == 0 {
                return;
            }
            let this = GetCurrentThreadId();
            let front = GetWindowThreadProcessId(GetForegroundWindow(), std::ptr::null_mut());
            let joined = front != 0 && front != this && AttachThreadInput(this, front, 1) != 0;
            SetForegroundWindow(window);
            if joined {
                AttachThreadInput(this, front, 0);
            }
        }
    }
}
