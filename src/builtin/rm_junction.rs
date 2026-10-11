//! rm-junction-guard (PreToolUse, Bash).
//!
//! pnpm's node_modules holds junctions (Windows links), and Git Bash's
//! `rm -r` can follow them and delete what they point to (the checkout a
//! `link:` dependency names). A recursive rm whose target is, or holds
//! within three levels, a node_modules folder is refused, pointing to
//! `cmd //c "rmdir /s /q ..."`, which removes only the link. Targets whose
//! place cannot be read (variables, globs, backquotes) are let through.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;

use crate::input::Input;
use crate::output::{Finding, Kind};
use crate::text::{any_line, matches, windows_path};

const ID: &str = "rm-junction-guard";
const START: &str = r"(^|[;&|(]|then|do|else)[[:space:]]*";

static RM: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&format!("{START}rm[[:space:]]")).unwrap());
static CALL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&format!("{START}rm[[:space:]][^;&|]*")).unwrap());
static HEAD: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^([;&|(]|then|do|else)?[[:space:]]*rm[[:space:]]+").unwrap());

/// Whether `dir` is, or holds within `depth` levels, a folder named
/// node_modules. Links are not followed, as `find` does not follow them.
fn holds_node_modules(dir: &Path, depth: usize) -> bool {
    if dir.file_name().is_some_and(|n| n == "node_modules") {
        return true;
    }
    if depth == 0 {
        return false;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return false;
    };
    entries.filter_map(Result::ok).any(|entry| {
        let is_dir = entry
            .file_type()
            .is_ok_and(|t| t.is_dir() && !t.is_symlink());
        is_dir && holds_node_modules(&entry.path(), depth - 1)
    })
}

fn is_real_dir(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|m| m.is_dir())
}

pub fn run(input: &Input) -> Vec<Finding> {
    if input.event() != "PreToolUse" || input.tool() != "Bash" {
        return Vec::new();
    }
    let command = input.command();
    if !any_line(&RM, &command) {
        return Vec::new();
    }
    let cwd = input.cwd();
    for call in matches(&CALL, &command) {
        let words = HEAD.replace(call, "");
        let mut is_recursive = false;
        let mut targets = Vec::new();
        for word in words.split_whitespace() {
            let word = word.replace(['"', '\''], "");
            if word == "--recursive" {
                is_recursive = true;
            } else if word.starts_with("--") {
            } else if word.starts_with('-') {
                is_recursive |= word.contains(['r', 'R']);
            } else {
                targets.push(word);
            }
        }
        if !is_recursive {
            continue;
        }
        for target in targets {
            if target.contains(['$', '*', '`']) {
                continue;
            }
            let bytes = target.as_bytes();
            let is_absolute = target.starts_with('/')
                || (bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':');
            let path = if is_absolute {
                PathBuf::from(windows_path(&target))
            } else {
                let base = if cwd.is_empty() {
                    ".".to_string()
                } else {
                    windows_path(&cwd)
                };
                Path::new(&base).join(&target)
            };
            if is_real_dir(&path) && holds_node_modules(&path, 3) {
                let reason = format!(
                    "rm -r の対象 {target} は node_modules を含む。pnpm の node_modules の junction を rm がたどると、\
                     link: 依存の先(兄弟の checkout 等)まで消しうる。cmd //c \"rmdir /s /q C:\\\\path\" で消す\
                     (junction はリンクだけ消える)。詳細 ~/.claude/docs/git.md の「部分 stage の単独検証と worktree の後片付け」"
                );
                return vec![Finding::new(ID, Kind::Deny, reason)];
            }
        }
    }
    Vec::new()
}
