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
    if !any_line(&GIT_PUSH, &command) {
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
