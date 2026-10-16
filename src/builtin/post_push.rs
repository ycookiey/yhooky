//! post-push-reminder (PostToolUse, Bash).
//!
//! Right after a `git push` that went through, names the repository's
//! after-push work (watching CI, building the screen flow catalog) with the
//! pushed commit. The note before the push (push-check-reminder) comes too
//! early to help once the push is done.

use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;

use crate::input::Input;
use crate::output::{Finding, Kind};
use crate::repo;
use crate::text::{any_line, matches, windows_path};

const ID: &str = "post-push-reminder";

pub static GIT_PUSH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(^|[^[:alnum:]_-])git([[:space:]]+-[^[:space:]]+([[:space:]]+[^-[:space:]][^[:space:]]*)?)*[[:space:]]+push([[:space:]]|$)",
    )
    .unwrap()
});
static QUOTED: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"'[^']*'|"(?:[^"\\]|\\.)*""#).unwrap());
/// What hands the next argument to a shell as a command.
static RUNS_ARG: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)(^|\s)(-c|/c|-command)\s*$").unwrap());

/// Whether `command` runs `git push`. Quoted text is data (a commit message
/// that mentions git push stopped the commit, 2026-10-15) unless it is
/// handed to a shell (`bash -c "..."`, `cmd /c "..."`, `pwsh -Command
/// "..."`). Heredoc bodies still count, as they may be run.
pub fn runs_git_push(command: &str) -> bool {
    runs(&GIT_PUSH, command)
}

/// Whether `command` runs what `re` matches, with quoted text read as data
/// in the same way as [`runs_git_push`].
pub fn runs(re: &Regex, command: &str) -> bool {
    let mut masked = String::with_capacity(command.len());
    let mut last = 0;
    for quoted in QUOTED.find_iter(command) {
        let before = &command[last..quoted.start()];
        masked.push_str(before);
        let inner = &quoted.as_str()[1..quoted.as_str().len() - 1];
        if RUNS_ARG.is_match(&command[..quoted.start()]) && runs(re, inner) {
            return true;
        }
        // Same length, no line breaks: lines keep their places.
        masked.extend(
            quoted
                .as_str()
                .chars()
                .map(|c| if c == '\n' { '\n' } else { 'x' }),
        );
        last = quoted.end();
    }
    masked.push_str(&command[last..]);
    any_line(re, &masked)
}

/// Only a push that went through prints `old..new  src -> dst`.
static PUSHED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"[0-9a-f]{7,40}\.\.\.?[0-9a-f]{7,40}[[:space:]]+[^[:space:]]+[[:space:]]+->")
        .unwrap()
});
static NEW_COMMIT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[0-9a-f]+\.\.\.?([0-9a-f]+)").unwrap());

/// The after-push scripts a repository has, as they are run.
const SCRIPTS: [&str; 2] = ["scripts/watch-push.sh", "scripts/flows-publish.sh"];

pub fn run(input: &Input) -> Vec<Finding> {
    if input.event() != "PostToolUse" || input.tool() != "Bash" {
        return Vec::new();
    }
    let command = input.command();
    if !runs_git_push(&command) {
        return Vec::new();
    }
    let output = input.response_text(&["stdout", "stderr", "output"]);
    let Some(last) = matches(&PUSHED, &output).pop() else {
        return Vec::new();
    };
    let Some(commit) = NEW_COMMIT.captures(last).map(|c| c[1].to_string()) else {
        return Vec::new();
    };
    let dir = repo::dir(input, repo::First::Cd);
    let base = windows_path(&dir);
    let scripts: Vec<String> = SCRIPTS
        .iter()
        .filter(|s| Path::new(&base).join(s).is_file())
        .map(|s| format!("{s} {commit}"))
        .collect();
    if scripts.is_empty() {
        return Vec::new();
    }
    let list: String = scripts.iter().map(|s| format!("「{s}」")).collect();
    vec![Finding::new(
        ID,
        Kind::Context,
        format!(
            "push 済み({commit}、{dir})。この repo の push 後の作業 {list} を、今それぞれ run_in_background で起動する。\
             片方だけで済ませない。"
        ),
    )]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tells_a_push_from_a_mention() {
        for command in [
            "git push",
            "cd /x && git push origin HEAD:main 2>&1 | tail -1",
            "git -C /x push",
            r#"bash -c "git push""#,
            "cmd /c 'git push'",
            r#"pwsh -Command "git -C x push --tags""#,
            "git commit -m 'msg' && git push",
            "cat <<'EOF' | bash\ngit push\nEOF",
        ] {
            assert!(runs_git_push(command), "{command}");
        }
        for command in [
            r#"git commit -m "feat: stop a git push once""#,
            "echo 'run git push later'",
            r#"gh release create v1 --notes "after git push""#,
            "git status",
        ] {
            assert!(!runs_git_push(command), "{command}");
        }
    }
}
