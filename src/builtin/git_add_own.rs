//! git-add-own-check (PreToolUse, Bash).
//!
//! stage-own-edit-reminder stops the first Edit or Write of a file that
//! already has uncommitted changes, but a file rewritten from the shell
//! (a script, `sed -i`, a redirect) never meets it, and `git add <file>`
//! would put it into the index whole, with another session's uncommitted
//! lines. So before `git add` takes a tracked file that differs from HEAD:
//!
//! - a file whose first change this session made with Edit or Write
//!   (stage-own-edit-reminder's mark) holds only this session's changes,
//!   and goes through;
//! - a file with a copy kept by `git stage-own-edit begin` is stopped each
//!   time: its own lines are staged with `git stage-own-edit stage`;
//! - any other file is stopped once per session and file, to look at its
//!   diff first; the same command sent again goes through.
//!
//! Only files named one by one are checked; `-A`, `.`, folders and globs
//! are not.

use std::fs;
use std::path::{Path, PathBuf};

use super::stage_own_edit::{begin_copy, git, own_mark, path_hash, session, unchanged};
use crate::docs;
use crate::input::Input;
use crate::output::{Finding, Kind};
use crate::repo::{parts_with_dirs, resolve};
use crate::text::windows_path;

const ID: &str = "git-add-own-check";

/// The words of a command, split at spaces outside quotes, quotes taken off.
fn words(head: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut word = String::new();
    let mut quote = None;
    let mut started = false;
    for ch in head.chars() {
        match quote {
            Some(q) if ch == q => quote = None,
            Some(_) => word.push(ch),
            None if ch == '"' || ch == '\'' => {
                quote = Some(ch);
                started = true;
            }
            None if ch.is_whitespace() => {
                if started {
                    out.push(std::mem::take(&mut word));
                    started = false;
                }
            }
            None => {
                word.push(ch);
                started = true;
            }
        }
    }
    if started {
        out.push(word);
    }
    out
}

/// The paths a `git … add …` part names, or nothing when the part is not
/// one. Options before `add` (`-C <dir>`, `-c <key=value>`) and after it
/// are passed over; after `--` every word is a path.
fn added_paths(head: &str) -> Vec<String> {
    let words = words(head);
    let mut rest = words.iter();
    if rest.next().map(String::as_str) != Some("git") {
        return Vec::new();
    }
    loop {
        match rest.next().map(String::as_str) {
            Some("-C" | "-c") => {
                rest.next();
            }
            Some(w) if w.starts_with('-') => {}
            Some("add") => break,
            _ => return Vec::new(),
        }
    }
    let mut paths = Vec::new();
    let mut options_end = false;
    for w in rest {
        if !options_end && w == "--" {
            options_end = true;
        } else if options_end || !w.starts_with('-') {
            paths.push(w.clone());
        }
    }
    paths
}

/// A named file `git add` would take: its repository's top and its path
/// from there. None for a folder, a glob, a file git does not track or one
/// outside a repository.
fn tracked(dir: &str, path: &str) -> Option<(PathBuf, String)> {
    if path.contains(['*', '?', '[']) {
        return None;
    }
    let full = PathBuf::from(windows_path(&resolve(dir, path).replace('\\', "/")));
    if !full.is_file() {
        return None;
    }
    let (parent, name) = (full.parent()?, full.file_name()?.to_string_lossy());
    let top = git(parent, &["rev-parse", "--show-toplevel"])?;
    let rel = git(
        parent,
        &["ls-files", "--full-name", "--error-unmatch", "--", &name],
    )
    .filter(|rel| !rel.is_empty())?;
    Some((PathBuf::from(windows_path(&top)), rel))
}

/// Notes that the file `hash` names was stopped in this session:
/// `Some(true)` the first time, `Some(false)` once noted, `None` when the
/// note cannot be written (then the file is not stopped, rather than
/// stopped every time).
fn note_stopped(input: &Input, hash: &str) -> Option<bool> {
    let mark = std::env::temp_dir()
        .join("claude-git-add-own-check")
        .join(session(input))
        .join(hash);
    if mark.exists() {
        return Some(false);
    }
    let written = mark
        .parent()
        .is_some_and(|parent| fs::create_dir_all(parent).is_ok())
        && fs::write(&mark, "").is_ok();
    written.then_some(true)
}

pub fn run(input: &Input) -> Vec<Finding> {
    if input.event() != "PreToolUse" || input.tool() != "Bash" {
        return Vec::new();
    }
    let mut kept = Vec::new();
    let mut unknown = Vec::new();
    for (dir, head) in parts_with_dirs(&input.command(), &input.cwd()) {
        for path in added_paths(&head) {
            let Some((top, rel)) = tracked(&dir, &path) else {
                continue;
            };
            if unchanged(&top, &rel) {
                continue;
            }
            let hash = path_hash(&rel);
            if own_mark(input, &hash).exists() {
                continue;
            }
            if begin_copy(&top, &hash).is_some_and(|copy| Path::new(&copy).is_file()) {
                kept.push(rel);
            } else if note_stopped(input, &hash) == Some(true) {
                unknown.push(rel);
            }
        }
    }
    if kept.is_empty() && unknown.is_empty() {
        return Vec::new();
    }
    let see = docs::pointer("、", "git.md", "");
    let mut parts = Vec::new();
    if !kept.is_empty() {
        let list = kept.join("・");
        parts.push(format!(
            "{list} は `git stage-own-edit begin` の写しがある。`git add` ではなく `git stage-own-edit stage <file>` で自分の行だけを stage する。\
             写しが前の作業の残りなら `git stage-own-edit abort <file>` で消してから出し直す。"
        ));
    }
    if !unknown.is_empty() {
        let list = unknown.join("・");
        parts.push(format!(
            "{list} は HEAD と差があり、この session が Edit/Write で最初に変えた file ではない(shell で書き換えたか、他 session の未コミットの変更がある)。\
             `git diff <file>` で、全部が自分の変更か確かめる。他の変更が混ざるなら、自分の行だけを stage する(git stage-own-edit{see})。\
             全部が自分の変更なら、同じコマンドを出し直せば通る(この file はこの session で一度だけ止める)。"
        ));
    }
    vec![Finding::new(ID, Kind::Deny, parts.join(""))]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_paths_git_add_names() {
        assert_eq!(added_paths("git add a.md b.md"), ["a.md", "b.md"]);
        assert_eq!(added_paths("git -C /c/r add -- -x.md"), ["-x.md"]);
        assert_eq!(added_paths("git -c core.x=1 add -v 'a b.md'"), ["a b.md"]);
        assert!(added_paths("git commit -m add").is_empty());
        assert!(added_paths("git stage-own-edit stage a.md").is_empty());
        assert!(added_paths("echo git add a.md").is_empty());
    }
}
