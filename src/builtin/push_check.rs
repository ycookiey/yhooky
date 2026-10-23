//! push-check-reminder (PreToolUse, Bash).
//!
//! Stops a `git push` the first time for each session, repository and HEAD,
//! with the checks to run before it; the same push sent again goes through.
//! A new HEAD (amend, rebase) stops it again. As a note added to the
//! context, the checklist arrived together with the push's result, after
//! the push (2026-10-11).
//!
//! A push chained after a `git commit` in the same command is stopped every
//! time, asking for the commit to be sent first: the hook sees the command
//! before it runs, so the commit made there would reach the remote unseen
//! by the checks before a push (this one, keyed by HEAD, and
//! private-word-guard, which reads the commits not yet pushed).
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

use super::backslash::split_heredocs;
use super::post_push::runs_git_push;
use super::public_writing::writing_changed;
use super::turn_state::all_heads;
use crate::docs;
use crate::input::Input;
use crate::output::{Finding, Kind};
use crate::public_repo::is_public;
use crate::text::windows_path;

const ID: &str = "push-check-reminder";

/// The folder pushed from: where the first `git push` of the command runs
/// (src/repo.rs).
pub(crate) fn push_dir(command: &str, cwd: &str) -> String {
    crate::repo::work_dir(command, cwd, runs_git_push)
}

/// Commands that change nothing a push leaves behind: changing folders,
/// and filtering the output.
static NOT_HELD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(cd|Set-Location|pushd|tail|head|grep|cat|tee|less|more|sed|awk|wc|sort|uniq|Select-Object|Select-String|Out-String)(\s|$)").unwrap()
});

/// What a stop of `command` holds back besides the push, as a sentence to
/// put first in the message, when it runs other commands too (`git add …
/// && git commit … && git push`). A message that speaks only of the push
/// lets the commit be taken as made: the push alone is sent again and goes
/// through with nothing new, or the work goes on without the commit.
/// Changing folders (`cd`) and filtering the output (`git push 2>&1 |
/// tail -3`) do not count; other setup (`rm`, `mkdir`) does, as it did not
/// run either.
pub(crate) fn held_back(command: &str) -> String {
    if all_heads(command)
        .iter()
        .all(|head| runs_git_push(head) || NOT_HELD.is_match(head))
    {
        return String::new();
    }
    "このコマンドの全体を止めた。`&&` や `;` でつないだほかのコマンド(add・commit など)も実行されていない。\
     出し直す前に `git status` と `git log -1` で確かめる。"
        .to_string()
}

static GIT_COMMIT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^git([[:space:]]+-[^[:space:]]+([[:space:]]+[^-[:space:]][^[:space:]]*)?)*[[:space:]]+commit([[:space:]]|$)",
    )
    .unwrap()
});

static QUOTED: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"'[^']*'|"(?:[^"\\]|\\.)*""#).unwrap());

/// Whether `command` makes a commit before it pushes. Heredoc bodies are
/// text (a commit message that mentions git push), and quoted arguments
/// count as one word (`git -C "a b" commit`). A command handed to a shell
/// (`bash -c "git commit … && git push"`) is not looked into.
pub(crate) fn commits_before_push(command: &str) -> bool {
    let (command, _) = split_heredocs(command);
    let heads = all_heads(&command);
    let Some(push) = heads.iter().position(|head| runs_git_push(head)) else {
        return false;
    };
    heads[..push]
        .iter()
        .any(|head| GIT_COMMIT.is_match(&QUOTED.replace_all(head, "x")))
}

const COMMIT_THEN_PUSH: &str = "commit と push を 1 つのコマンドでつないでいる。hook はコマンドが動く前に見るので、\
     その場で作る commit を push 前の確認(この確認と private-word-guard)が調べられない。\
     このコマンドの全体を止めた(commit などのつないだコマンドも実行されていない)。commit までを先に出し、push は別のコマンドで出す。\
     つないだままなら、出し直しても毎回止める。";

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
    if commits_before_push(&command) {
        return vec![Finding::new(ID, Kind::Deny, COMMIT_THEN_PUSH)];
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
    let autocrlf = match docs::path("git.md") {
        Some(path) => format!(" {path} の手順(コミットした中身で確かめる)"),
        None => "、コミットした中身で確かめる".to_string(),
    };
    let mut message = format!(
        "{}push 前の確認(この repo のこの commit で初回の push なので一度止めた。済んだら同じコマンドを出し直せば通る): \
         このrepoのCI workflowの run: 行(package.jsonのscript: typecheck / lint / format:check / test / build 等)をそのまま回したか。\
         toolの単体呼び出しで代用しない(例: eslint単体は同じlint scriptの前段oxlintを飛ばす)。\
         Windowsのautocrlfでformat:checkが落ちる場合は{autocrlf}。\
         push 先の決まり(他の session への知らせ・ユーザーの了承)も満たしたか。(push後の作業は post-push-reminder が push の直後に知らせる)",
        held_back(&command)
    );
    // `repo` is the top level and HEAD, one per line.
    if let Some(root) = repo.as_deref().and_then(|r| r.lines().next())
        && is_public(Path::new(&windows_path(root)))
        && writing_changed(root)
    {
        message.push_str(&public_review());
    }
    vec![Finding::new(ID, Kind::Deny, message)]
}

fn public_review() -> String {
    let aspects = match docs::path("public-repo.md") {
        Some(path) => format!(" {path} の観点"),
        None => "、次の観点".to_string(),
    };
    format!(
        "\nこの repo は public で、push する範囲に書き物の変更がある。push する範囲の書き物の変更を、\
         レビュー用の subagent に渡し、書き手を知らない想定利用者として読ませて{aspects}(書き手の環境・作業の経緯・\
         会話の中だけの呼び名を前提にしていないか・言語がそろっているか・普通の public repo として標準的な形か・\
         個人の具体を書いていないか)でレビューし、指摘を直してから出し直す。対象は README・md・コード内のコメント・package の説明・commit のメッセージ。"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn says_what_else_a_stop_holds_back() {
        for command in [
            "git add a.rs && git commit -m 'x' && git push origin main",
            "git commit -am x; git push",
            "git push && gh pr create --fill",
            "rm -r dist && git push",
        ] {
            assert!(!held_back(command).is_empty(), "{command}");
        }
        for command in [
            "git push origin main",
            "cd /c/x && git push",
            "git -C /c/x push --force-with-lease",
            "git push 2>&1",
            "git push 2>&1 | tail -3",
        ] {
            assert_eq!(held_back(command), "", "{command}");
        }
    }

    #[test]
    fn finds_a_commit_before_the_push() {
        for command in [
            "git add a.rs && git commit -m 'x' && git push origin main",
            "git commit -am x; git push",
            "cd /c/x && git -C /c/x commit --amend --no-edit && git push --force-with-lease",
            "git commit -F - <<'EOF'\nmessage\nEOF\ngit push",
            "git -C \"C:/a b\" commit -m x && git push",
        ] {
            assert!(commits_before_push(command), "{command}");
        }
        for command in [
            "git push origin main",
            "git push && git commit -m x",
            "git log -1 && git push",
            "git commit -m 'then git push'",
            "echo 'git commit' && git push",
            "git commit -F - <<'EOF'\nfeat: x\n\ngit push is stopped\nEOF",
        ] {
            assert!(!commits_before_push(command), "{command}");
        }
    }

    #[test]
    fn stops_a_commit_and_push_every_time() {
        let dir = tempfile::tempdir().unwrap();
        // SAFETY: only this test reads the variable.
        unsafe { std::env::set_var("PUSH_CHECK_STATE_DIR", dir.path()) };
        let raw = serde_json::json!({
            "hook_event_name": "PreToolUse",
            "tool_name": "Bash",
            "session_id": "s",
            "cwd": dir.path(),
            "tool_input": { "command": "git commit -am x && git push" },
        });
        let input = Input::parse(&raw.to_string()).unwrap();
        for _ in 0..2 {
            let found = run(&input);
            assert_eq!(found.len(), 1);
            assert!(matches!(found[0].kind, Kind::Deny));
            assert!(found[0].message.starts_with("commit と push"));
        }
    }
}
