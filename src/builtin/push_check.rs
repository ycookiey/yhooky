//! push-check-reminder (PreToolUse, Bash).
//!
//! Stops a `git push` the first time for each session, repository and HEAD,
//! with the checks to run before it; the same push sent again goes through.
//! A new HEAD (amend, rebase) stops it again. As a note added to the
//! context, the checklist arrived together with the push's result, after
//! the push (2026-10-11).
//!
//! Which pushes were stopped is kept as empty files in
//! `$PUSH_CHECK_STATE_DIR`, else `$TMPDIR/claude-push-checked`, named by the
//! first 16 hex digits of the sha1 of `<session> <repo root>\n<HEAD>`, or
//! of `<session> <cwd> <command>` outside a repository: the same files the
//! shell version of this hook keeps, so both agree while sessions move over.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::LazyLock;

use regex::Regex;

use super::post_push::runs_git_push;
use super::public_writing::writing_changed;
use crate::input::Input;
use crate::output::{Finding, Kind};
use crate::public_repo::is_public;
use crate::text::{matches, strip_quote, windows_path};

const ID: &str = "push-check-reminder";

static CD: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(^|[;&|[:space:]])cd[[:space:]]+[^;&|[:space:]]+").unwrap());
static CD_PREFIX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^.*cd[[:space:]]+").unwrap());
static GIT_C: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"git[[:space:]]+-C[[:space:]]+[^[:space:]]+").unwrap());
static GIT_C_PREFIX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^git[[:space:]]+-C[[:space:]]+").unwrap());

/// The folder pushed from: the last `cd <dir>`, else the last
/// `git -C <dir>`, relative to the hook's cwd; else the cwd.
pub(crate) fn push_dir(command: &str, cwd: &str) -> String {
    let found = matches(&CD, command)
        .pop()
        .map(|m| CD_PREFIX.replace(m, "").into_owned())
        .or_else(|| {
            matches(&GIT_C, command)
                .pop()
                .map(|m| GIT_C_PREFIX.replace(m, "").into_owned())
        })
        .unwrap_or_default();
    let dir = strip_quote(&found, '"').to_string();
    let bytes = dir.as_bytes();
    if dir.is_empty() {
        cwd.to_string()
    } else if dir.starts_with('/')
        || (bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':')
    {
        dir
    } else {
        format!("{cwd}/{dir}")
    }
}

fn state_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("PUSH_CHECK_STATE_DIR") {
        return PathBuf::from(windows_path(&dir));
    }
    let tmp = match std::env::var("TMPDIR") {
        // Git Bash's /tmp is the user's temporary folder.
        Ok(dir) if dir == "/tmp" || dir.is_empty() => std::env::temp_dir(),
        Ok(dir) => PathBuf::from(windows_path(&dir)),
        Err(_) => std::env::temp_dir(),
    };
    tmp.join("claude-push-checked")
}

pub fn run(input: &Input) -> Vec<Finding> {
    if input.event() != "PreToolUse" || input.tool() != "Bash" {
        return Vec::new();
    }
    let command = input.command();
    if !runs_git_push(&command) {
        return Vec::new();
    }
    let cwd = input.cwd();
    let dir = push_dir(&command, &cwd);
    let session = input.text("session_id");
    let repo = Command::new("git")
        .args([
            "-C",
            &windows_path(&dir),
            "rev-parse",
            "--show-toplevel",
            "HEAD",
        ])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map(|out| {
            String::from_utf8_lossy(&out.stdout)
                .trim_end_matches(['\n', '\r'])
                .replace("\r\n", "\n")
        });
    let key = match &repo {
        Some(repo) => format!("{session} {repo}"),
        None => format!("{session} {cwd} {command}"),
    };
    let hash = sha1_smol::Sha1::from(key.as_bytes()).digest().to_string();
    let state = state_dir();
    let mark = state.join(&hash[..16]);
    if mark.exists() {
        return Vec::new();
    }
    if fs::create_dir_all(&state).is_err() || fs::write(&mark, "").is_err() {
        // Without a place to remember it, a stop would come back every time.
        return Vec::new();
    }
    let mut message = String::from(
        "push 前の確認(この repo のこの commit で初回の push なので一度止めた。済んだら同じコマンドを出し直せば通る): \
         このrepoのCI workflowの run: 行(package.jsonのscript: typecheck / lint / format:check / test / build 等)をそのまま回したか。\
         toolの単体呼び出しで代用しない(例: eslint単体は同じlint scriptの前段oxlintを飛ばす)。\
         Windowsのautocrlfでformat:checkが落ちる場合は ~/.claude/docs/git.md の手順(コミットした中身で確かめる)。\
         push 先の決まり(他の session への知らせ・ユーザーの了承)も満たしたか。(push後の作業は post-push-reminder が push の直後に知らせる)",
    );
    // `repo` is the top level and HEAD, one per line.
    if let Some(root) = repo.as_deref().and_then(|r| r.lines().next())
        && is_public(Path::new(&windows_path(root)))
        && writing_changed(root)
    {
        message.push_str(PUBLIC_REVIEW);
    }
    vec![Finding::new(ID, Kind::Deny, message)]
}

const PUBLIC_REVIEW: &str = "\nこの repo は public で、push する範囲に書き物の変更がある。push する範囲の書き物の変更を、\
     レビュー用の subagent に渡して ~/.claude/docs/public-repo.md の観点(言語がそろっているか・普通の public repo として標準的な形か・\
     個人の具体を書いていないか)でレビューし、指摘を直してから出し直す。対象は README・md・コード内のコメント・package の説明・commit のメッセージ。";
