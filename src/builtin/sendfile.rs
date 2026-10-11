//! sendfile-to-ysidepaney (PreToolUse, SendUserFile).
//!
//! The user sees images in ysidepaney, open beside the conversation, and
//! videos on their own machine. The images SendUserFile was about to send
//! (png, jpg, jpeg, gif, webp, bmp) are posted there with `ysidepaney post
//! image`; each video (mp4, mov, webm, mkv, avi, m4v, wmv) that exists is
//! shown selected in a minimized Explorer window (Windows only,
//! src/explorer.rs), by `yhooky show-file` started in the background.
//! SendUserFile is then
//! refused, and the other files, with any image or video that could not be
//! handled, are left to be sent again on their own. Nothing is done when no
//! file could be handled. YSIDEPANEY_BIN replaces the ysidepaney command,
//! and EXPLORER_BIN, when set, a command run with the video's path in place
//! of Explorer (both for tests).

use std::path::Path;
use std::process::{Command, Stdio};

use serde_json::Value;

use crate::input::Input;
use crate::output::{Finding, Kind};

const ID: &str = "sendfile-to-ysidepaney";
const IMAGE_TYPES: [&str; 6] = ["png", "jpg", "jpeg", "gif", "webp", "bmp"];
const VIDEO_TYPES: [&str; 7] = ["mp4", "mov", "webm", "mkv", "avi", "m4v", "wmv"];
const TITLE_CHARS: usize = 80;

/// Whether `bin` can be run: a path to a file, or a name found on PATH.
fn exists(bin: &str) -> bool {
    if bin.contains(['/', '\\']) {
        return Path::new(bin).is_file();
    }
    let exts: Vec<String> = std::env::var("PATHEXT")
        .map(|v| v.split(';').map(|e| e.to_lowercase()).collect())
        .unwrap_or_default();
    std::env::var_os("PATH").is_some_and(|paths| {
        std::env::split_paths(&paths).any(|dir| {
            dir.join(bin).is_file() || exts.iter().any(|e| dir.join(format!("{bin}{e}")).is_file())
        })
    })
}

fn has_type(file: &str, types: &[&str]) -> bool {
    let lower = file.to_lowercase();
    types.iter().any(|t| lower.ends_with(&format!(".{t}")))
}

/// `file` joined to `cwd` unless it is absolute already.
fn full_path(file: &str, cwd: &str) -> String {
    let bytes = file.as_bytes();
    let is_absolute = file.starts_with('/')
        || (bytes.len() >= 3
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && matches!(bytes[2], b'/' | b'\\'));
    if is_absolute {
        file.to_string()
    } else {
        format!("{cwd}/{file}")
    }
}

fn file_name(path: &str) -> String {
    path.rsplit(['/', '\\']).next().unwrap_or(path).to_string()
}

/// `path` as Windows writes it: `/c/x/y` and `C:/x/y` as `C:\x\y`.
fn windows_path(path: &str) -> String {
    let bytes = path.as_bytes();
    let path = if bytes.len() >= 3
        && bytes[0] == b'/'
        && bytes[1].is_ascii_alphabetic()
        && bytes[2] == b'/'
    {
        format!("{}:{}", (bytes[1] as char).to_ascii_uppercase(), &path[2..])
    } else {
        path.to_string()
    };
    path.replace('/', "\\")
}

fn post_image(bin: &str, path: &str, title: &str, session: &str) -> bool {
    let mut command = Command::new(bin);
    command.args(["post", "image", path, "--title", title]);
    if !session.is_empty() {
        command.args(["--session", session]);
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// Starts `yhooky show-file <path>` and leaves it running, so the hook does
/// not wait for the window.
#[cfg(windows)]
fn start_show_file(path: &str) -> bool {
    use std::ffi::c_void;
    use std::os::windows::process::CommandExt;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetStdHandle(which: u32) -> *mut c_void;
        fn GetHandleInformation(handle: *mut c_void, flags: *mut u32) -> i32;
        fn SetHandleInformation(handle: *mut c_void, mask: u32, flags: u32) -> i32;
    }
    const STD_HANDLES: [u32; 3] = [-10i32 as u32, -11i32 as u32, -12i32 as u32];
    const HANDLE_FLAG_INHERIT: u32 = 1;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    // A child started on Windows gets every inheritable handle, the pipes
    // the hook answers on among them, and whoever reads those pipes waits
    // until the child ends. They are made uninheritable while it starts.
    let mut kept = Vec::new();
    for which in STD_HANDLES {
        // SAFETY: plain calls on this process's standard handles.
        unsafe {
            let handle = GetStdHandle(which);
            let mut flags = 0;
            if !handle.is_null()
                && handle as isize != -1
                && GetHandleInformation(handle, &mut flags) != 0
                && flags & HANDLE_FLAG_INHERIT != 0
                && SetHandleInformation(handle, HANDLE_FLAG_INHERIT, 0) != 0
            {
                kept.push(handle);
            }
        }
    }
    let started = Command::new(exe)
        .args(["show-file", path])
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .is_ok();
    for handle in kept {
        // SAFETY: as above; the flag is put back as it was.
        unsafe { SetHandleInformation(handle, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT) };
    }
    started
}

#[cfg(not(windows))]
fn start_show_file(_path: &str) -> bool {
    false
}

/// Shows the video at `path`: with the EXPLORER_BIN command when it is set,
/// in Explorer otherwise.
fn show_video(path: &str) -> bool {
    match std::env::var("EXPLORER_BIN") {
        Ok(bin) => {
            exists(&bin)
                && Command::new(&bin)
                    .arg(path)
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()
                    .is_ok_and(|s| s.success())
        }
        Err(_) => start_show_file(path),
    }
}

pub fn run(input: &Input) -> Vec<Finding> {
    if input.event() != "PreToolUse" || input.tool() != "SendUserFile" {
        return Vec::new();
    }
    let cwd = input.cwd();
    let session = input.text("session_id");
    let caption: String = input
        .text("tool_input.caption")
        .lines()
        .next()
        .unwrap_or_default()
        .chars()
        .take(TITLE_CHARS)
        .collect();
    let mut images = Vec::new();
    let mut videos = Vec::new();
    let mut rest = Vec::new();
    let files = input.get("tool_input.files").and_then(Value::as_array);
    for file in files
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter(|f| !f.is_empty())
    {
        if has_type(file, &IMAGE_TYPES) {
            images.push(file.to_string());
        } else if has_type(file, &VIDEO_TYPES) {
            videos.push(file.to_string());
        } else {
            rest.push(file.to_string());
        }
    }

    let mut posted = Vec::new();
    if !images.is_empty() {
        let bin = std::env::var("YSIDEPANEY_BIN").unwrap_or_else(|_| "ysidepaney".into());
        let ready = exists(&bin);
        for file in &images {
            let path = full_path(file, &cwd);
            let name = file_name(&path);
            let mut title = if caption.is_empty() {
                name.clone()
            } else {
                caption.clone()
            };
            if !caption.is_empty() && images.len() > 1 {
                title = format!("{caption} ({name})");
            }
            if ready && post_image(&bin, &path, &title, &session) {
                posted.push(name);
            } else {
                rest.push(file.clone());
            }
        }
    }

    let mut shown = Vec::new();
    for file in &videos {
        let path = windows_path(&full_path(file, &cwd));
        // Explorer opens another folder for a path that is not there.
        if Path::new(&path).is_file() && show_video(&path) {
            shown.push(path);
        } else {
            rest.push(file.clone());
        }
    }

    if posted.is_empty() && shown.is_empty() {
        return Vec::new();
    }
    let mut reason = String::new();
    if !posted.is_empty() {
        reason.push_str(&format!(
            "画像 {} 枚({})を ysidepaney に置いた。画像は SendUserFile で送らない。",
            posted.len(),
            posted.join(" ")
        ));
    }
    if !shown.is_empty() {
        reason.push_str(&format!(
            "動画 {} 本({})を Explorer で選んだ状態で開き、最小化してタスクバーに置いた。動画は SendUserFile で送らない。",
            shown.len(),
            shown.join(" ")
        ));
    }
    reason.push_str("会話では、何を確かめるものかを文で伝える。");
    if !rest.is_empty() {
        reason.push_str(&format!(
            " 残り({})を送るなら、それだけで SendUserFile をもう一度呼ぶ。",
            rest.join(" ")
        ));
    }
    vec![Finding::new(ID, Kind::Deny, reason)]
}
