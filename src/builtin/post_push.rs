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
use crate::text::{matches, windows_path};

const ID: &str = "post-push-reminder";

pub static GIT_PUSH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(^|[^[:alnum:]_-])git([[:space:]]+-[^[:space:]]+([[:space:]]+[^-[:space:]][^[:space:]]*)?)*[[:space:]]+push([[:space:]]|$)",
    )
    .unwrap()
});
/// Whether `command` runs `git push`, as src/shell.rs reads it: quoted
/// text (a commit message that mentions git push) and heredoc bodies that
/// only feed a file or a program are data.
pub fn runs_git_push(command: &str) -> bool {
    runs(&GIT_PUSH, command)
}

/// Whether one command, as [`crate::shell::commands`] gives it, is itself
/// a `git push` (not one handed to a shell inside it). A `#` is text here:
/// the commands given hold no comment, and a part
/// [`crate::shell::as_text_parts`] gives may hold a `#` inside a word
/// (`echo $(date)#& git push`).
pub fn is_git_push(command: &str) -> bool {
    GIT_PUSH.is_match(&crate::shell::without_quotes(command))
}

/// Whether `command` runs what `re` matches, read as for
/// [`runs_git_push`].
pub fn runs(re: &Regex, command: &str) -> bool {
    crate::shell::runs(re, command)
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

/// The folder the first `git push` of `command` ran in (src/repo.rs).
pub(crate) fn post_push_dir(command: &str, cwd: &str) -> String {
    repo::work_dir(command, cwd, is_git_push)
}

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
    let dir = post_push_dir(&command, &input.cwd());
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
