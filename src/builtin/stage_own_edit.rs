//! stage-own-edit-reminder (PreToolUse, Edit|Write).
//!
//! Editing a file that already has uncommitted changes (another session's
//! or tool's) leaves no way to commit only this edit later, unless a copy
//! was kept first with `git stage-own-edit begin <file>`. The first edit of
//! a tracked file that differs from HEAD and has no such copy is stopped,
//! once per session and file; the same edit sent again goes through. A file
//! this session first edits while it matches HEAD is remembered too: its
//! later changes are this session's own, so its next edits are not stopped
//! (2026-10-12: a second Edit of the same file in one reply was stopped by the
//! changes the first had just made, and the stop went unnoticed). As a
//! note added to the context, the reminder arrived with the edit's result,
//! after the file had changed, and copies were not kept (2026-10-11). The
//! copy is where git-stage-own-edit keeps it:
//! `<git-path stage-own-edit>/<sha1 of the repo-relative path>`.
//!
//! The copies are one per repository, not per session, and one stays behind
//! when `stage` fails or the file is committed another way. A copy counts
//! only while it matches the file byte for byte, as it does right after
//! `begin`; the edit that passes on it is remembered, so the edits after it
//! pass on the mark (2026-10-15: a copy left by a failed `stage` four days
//! earlier let another session's first edit through, and its commit took
//! the other session's lines along).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::docs;
use crate::input::Input;
use crate::output::{Finding, Kind};
use crate::text::windows_path;

const ID: &str = "stage-own-edit-reminder";

/// What `git -C <dir> <args>` printed, trimmed, when it succeeded.
pub(crate) fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// The key a file is kept under, here and by git-stage-own-edit: the sha1
/// of its path from the repository's top.
pub(crate) fn path_hash(rel: &str) -> String {
    sha1_smol::Sha1::from(rel.as_bytes()).digest().to_string()
}

/// The session's id, or "none" when the hook was not given one.
pub(crate) fn session(input: &Input) -> String {
    match input.text("session_id") {
        s if s.is_empty() => "none".to_string(),
        s => s,
    }
}

/// Where this check notes that the session's changes to a file are its
/// own. git-add-own-check reads the same mark.
pub(crate) fn own_mark(input: &Input, hash: &str) -> PathBuf {
    std::env::temp_dir()
        .join("claude-stage-own-edit-reminder")
        .join(session(input))
        .join(hash)
}

/// Whether `rel` in the repository at `top` is as in HEAD.
pub(crate) fn unchanged(top: &Path, rel: &str) -> bool {
    Command::new("git")
        .arg("-C")
        .arg(top)
        .args(["diff", "--quiet", "HEAD", "--", rel])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// Where `git stage-own-edit begin` keeps its copy of the file `hash`
/// names, in the repository at `top`.
pub(crate) fn begin_copy(top: &Path, hash: &str) -> Option<PathBuf> {
    let store = git(top, &["rev-parse", "--git-path", "stage-own-edit"])?;
    let store = PathBuf::from(windows_path(&store));
    let store = if store.is_absolute() {
        store
    } else {
        top.join(store)
    };
    Some(store.join(hash))
}

pub fn run(input: &Input) -> Vec<Finding> {
    let tool = input.tool();
    if input.event() != "PreToolUse" || (tool != "Edit" && tool != "Write") {
        return Vec::new();
    }
    let path = input.text("tool_input.file_path");
    if path.is_empty() {
        return Vec::new();
    }
    let path = PathBuf::from(windows_path(&path.replace('\\', "/")));
    if !path.is_file() {
        return Vec::new();
    }
    let (Some(dir), Some(name)) = (path.parent(), path.file_name()) else {
        return Vec::new();
    };
    let Some(top) = git(dir, &["rev-parse", "--show-toplevel"]) else {
        return Vec::new();
    };
    let name = name.to_string_lossy();
    let rel = match git(
        dir,
        &["ls-files", "--full-name", "--error-unmatch", "--", &name],
    ) {
        Some(rel) if !rel.is_empty() => rel,
        _ => return Vec::new(),
    };
    let top = PathBuf::from(&top);
    let hash = path_hash(&rel);
    let mark = own_mark(input, &hash);
    let remember = || {
        mark.parent()
            .is_some_and(|parent| fs::create_dir_all(parent).is_ok())
            && fs::write(&mark, "").is_ok()
    };
    if mark.exists() {
        return Vec::new();
    }
    if unchanged(&top, &rel) {
        // This session's edit is the first change: later ones are its own.
        let _ = remember();
        return Vec::new();
    }
    let Some(copy) = begin_copy(&top, &hash) else {
        return Vec::new();
    };
    let kept = fs::read(&copy).ok();
    if kept.is_some() && kept == fs::read(&path).ok() {
        // Kept by `begin` just now: the edits from here on are this one's.
        let _ = remember();
        return Vec::new();
    }
    if !remember() {
        // Without a place to remember it, a stop would come back every time.
        return Vec::new();
    }
    let stale = if kept.is_some() {
        "store にある写しは今の file と違う(以前の begin の残りか、他 session の begin の後に変わったもの)。"
    } else {
        ""
    };
    let see = docs::pointer("。", "git.md", "");
    vec![Finding::new(
        ID,
        Kind::Deny,
        format!(
            "{rel} には HEAD からの未コミットの変更がある(他 session の作業かもしれない)。この file の初回の編集なので一度止めた。{stale}\
             今回の編集を後で単独で commit するなら、先に `git stage-own-edit begin {rel}` を repo の top で実行してから、\
             同じ編集を出し直す(commit 時は `git stage-own-edit stage {rel}` で自分の行だけを stage する{see})。\
             単独で commit しないなら、そのまま出し直せば通る。"
        ),
    )]
}
