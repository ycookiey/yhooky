//! sendfile-to-ysidepaney (PreToolUse, SendUserFile).
//!
//! The user sees images in ysidepaney, open beside the conversation. The
//! images SendUserFile was about to send (png, jpg, jpeg, gif, webp, bmp)
//! are posted there with `ysidepaney post image`, and SendUserFile is
//! refused; other files are left to be sent again on their own. Nothing is
//! done without images, without ysidepaney, or when posting fails.
//! YSIDEPANEY_BIN replaces the command (for tests).

use std::path::Path;
use std::process::{Command, Stdio};

use serde_json::Value;

use crate::input::Input;
use crate::output::{Finding, Kind};

const ID: &str = "sendfile-to-ysidepaney";
const IMAGE_TYPES: [&str; 6] = ["png", "jpg", "jpeg", "gif", "webp", "bmp"];
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

pub fn run(input: &Input) -> Vec<Finding> {
    if input.event() != "PreToolUse" || input.tool() != "SendUserFile" {
        return Vec::new();
    }
    let bin = std::env::var("YSIDEPANEY_BIN").unwrap_or_else(|_| "ysidepaney".into());
    if !exists(&bin) {
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
    let mut others = Vec::new();
    let files = input.get("tool_input.files").and_then(Value::as_array);
    for file in files
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter(|f| !f.is_empty())
    {
        let lower = file.to_lowercase();
        if IMAGE_TYPES
            .iter()
            .any(|t| lower.ends_with(&format!(".{t}")))
        {
            images.push(file.to_string());
        } else {
            others.push(file.to_string());
        }
    }
    if images.is_empty() {
        return Vec::new();
    }
    let mut posted = Vec::new();
    for file in &images {
        let bytes = file.as_bytes();
        let is_absolute = file.starts_with('/')
            || (bytes.len() >= 3
                && bytes[0].is_ascii_alphabetic()
                && bytes[1] == b':'
                && matches!(bytes[2], b'/' | b'\\'));
        let path = if is_absolute {
            file.clone()
        } else {
            format!("{cwd}/{file}")
        };
        let name = path.rsplit(['/', '\\']).next().unwrap_or(&path).to_string();
        let mut title = if caption.is_empty() {
            name.clone()
        } else {
            caption.clone()
        };
        if !caption.is_empty() && images.len() > 1 {
            title = format!("{caption} ({name})");
        }
        let mut command = Command::new(&bin);
        command.args(["post", "image", &path, "--title", &title]);
        if !session.is_empty() {
            command.args(["--session", &session]);
        }
        let ok = command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success());
        if !ok {
            return Vec::new();
        }
        posted.push(name);
    }
    let mut reason = format!(
        "画像 {} 枚({})を ysidepaney に置いた。画像は SendUserFile で送らない。会話では、何を確かめる画像かを文で伝える。",
        posted.len(),
        posted.join(" ")
    );
    if !others.is_empty() {
        reason.push_str(&format!(
            " 画像以外({})を送るなら、それだけで SendUserFile をもう一度呼ぶ。",
            others.join(" ")
        ));
    }
    vec![Finding::new(ID, Kind::Deny, reason)]
}
