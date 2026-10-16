//! public-repo-writing-reminder (PreToolUse).
//!
//! The form an existing repository has so far is not taken as the standard:
//! its text is compared with public repositories of its kind instead.
//! Creating a PR or a release is a point of its own, since their text
//! cannot be made private afterwards.
//!
//! A repository and its worktrees count as one repository, named by their
//! shared git config.
//!
//! Which were noted is kept as empty files in `$PUBLIC_WRITING_STATE_DIR`,
//! else `<temp>/claude-public-writing`.

use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::LazyLock;

use regex::Regex;

use super::post_push::runs;
use super::push_check::push_dir;
use crate::input::Input;
use crate::output::{Finding, Kind};
use crate::public_repo::{git_config, github_public, is_public};
use crate::text::{matches, strip_quote, windows_path};

const ID: &str = "public-repo-writing-reminder";

const WRITE_NOTE: &str = "この repo は public(誰でも読める)。書き物(README・コード内のコメント・PR・commit・package の説明)の言語は、\
     repo の README と既存の commit にそろえる。普通の public repo として標準的な形か、よく使われている同種の repo と比べて確かめる\
     (今の repo の書き方は根拠にしない)。詳しくは ~/.claude/docs/public-repo.md。";

const PUBLISH_NOTE: &str = "この repo は public(誰でも読める)。PR・Release の説明文は、後から非公開にできない。\
     言語は repo の README と既存の commit にそろえ、普通の public repo として標準的な形か、よく使われている同種の repo と比べて\
     確かめてから出す(今の repo の書き方は根拠にしない)。詳しくは ~/.claude/docs/public-repo.md。";

static PUBLISH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(^|[^[:alnum:]_-])gh[[:space:]]+(pr|release)[[:space:]]+create([[:space:]]|$)")
        .unwrap()
});
static REPO_FLAG: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(^|[[:space:]])(-R|--repo)(=|[[:space:]]+)["']?[^[:space:]"']+"#).unwrap()
});
static REPO_FLAG_PREFIX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"^[[:space:]]*(-R|--repo)(=|[[:space:]]+)"#).unwrap());

/// `owner/repo` given to gh with `-R`/`--repo` (the last one), if any. A
/// host prefix (`github.com/owner/repo`) is dropped.
fn repo_flag(command: &str) -> Option<String> {
    let found = matches(&REPO_FLAG, command).pop()?;
    let value = REPO_FLAG_PREFIX.replace(found.trim_start(), "");
    let value = strip_quote(strip_quote(&value, '"'), '\'');
    let value = value.strip_prefix("github.com/").unwrap_or(value);
    let mut parts = value.split('/');
    match (parts.next(), parts.next(), parts.next()) {
        (Some(owner), Some(repo), None) if !owner.is_empty() && !repo.is_empty() => {
            Some(format!("{owner}/{repo}"))
        }
        _ => None,
    }
}

fn first_time(session: &str, point: &str, repo: &str) -> bool {
    let dir = crate::once::state_dir("PUBLIC_WRITING_STATE_DIR", "claude-public-writing");
    crate::once::first_time(&dir, &format!("{session}\n{point}\n{repo}"))
}

/// The repository a path lies in, named by its git config (one name for a
/// repository and its worktrees), when it is public.
fn public_repo_of(path: &Path) -> Option<String> {
    let config = git_config(path)?;
    is_public(path).then(|| config.to_string_lossy().into_owned())
}

pub fn run(input: &Input) -> Vec<Finding> {
    if input.event() != "PreToolUse" {
        return Vec::new();
    }
    let session = input.text("session_id");
    let tool = input.tool();
    let (point, repo, note) = match tool.as_str() {
        "Edit" | "Write" => {
            let path = input.text("tool_input.file_path");
            if path.is_empty() {
                return Vec::new();
            }
            let path = windows_path(&path.replace('\\', "/"));
            let Some(repo) = public_repo_of(Path::new(&path)) else {
                return Vec::new();
            };
            ("write", repo, WRITE_NOTE)
        }
        "Bash" | "PowerShell" => {
            let command = input.command();
            if !runs(&PUBLISH, &command) {
                return Vec::new();
            }
            let repo = match repo_flag(&command) {
                Some(name) => github_public(&name).then(|| format!("github:{name}")),
                None => {
                    let dir = windows_path(&push_dir(&command, &input.cwd()));
                    public_repo_of(Path::new(&dir))
                }
            };
            let Some(repo) = repo else {
                return Vec::new();
            };
            ("publish", repo, PUBLISH_NOTE)
        }
        _ => return Vec::new(),
    };
    if !first_time(&session, point, &repo) {
        return Vec::new();
    }
    vec![Finding::new(ID, Kind::Context, note)]
}

/// Whether a changed file is text for readers: a README, a changelog and
/// the like, any Markdown, or a package's description.
fn is_writing_file(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    let upper = name.to_ascii_uppercase();
    ["README", "CHANGELOG", "CONTRIBUTING", "LICENSE"]
        .iter()
        .any(|p| upper.starts_with(p))
        || upper.ends_with(".MD")
        || upper.ends_with(".MDX")
        || name == "package.json"
}

/// Whether an added line is a comment. `#` counts only as a comment
/// starter (`# note`), not as a Rust attribute or a C directive.
fn is_comment_line(line: &str) -> bool {
    let line = line.trim_start();
    line.starts_with("//")
        || line.starts_with("/*")
        || line.starts_with("<!--")
        || line == "#"
        || line.starts_with("# ")
        || line == "*"
        || line.starts_with("* ")
        || line.starts_with("*/")
}

/// Whether `diff` (`git diff -U0`) changes text for readers.
fn diff_has_writing(diff: &str) -> bool {
    diff.lines().any(|line| {
        if let Some(path) = line.strip_prefix("+++ b/") {
            is_writing_file(path)
        } else if line.starts_with("+++") {
            false
        } else if let Some(added) = line.strip_prefix('+') {
            is_comment_line(added)
        } else {
            false
        }
    })
}

/// Whether the commits about to be pushed from `root` change text for
/// readers. They are the commits since the upstream, else since origin's
/// default branch; when neither is known, they are counted as changing it,
/// so that a review is not skipped.
pub(crate) fn writing_changed(root: &str) -> bool {
    for base in ["@{upstream}...HEAD", "refs/remotes/origin/HEAD...HEAD"] {
        let out = Command::new("git")
            .args(["-C", &windows_path(root), "diff", "-U0", "--no-color"])
            // The reading below needs these prefixes, whatever the config says.
            .args(["--src-prefix=a/", "--dst-prefix=b/", base])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output();
        if let Ok(out) = out
            && out.status.success()
        {
            return diff_has_writing(&String::from_utf8_lossy(&out.stdout));
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_repo_flag() {
        assert_eq!(
            repo_flag("gh pr create -R o/r --title x").as_deref(),
            Some("o/r")
        );
        assert_eq!(
            repo_flag("gh release create v1 --repo=github.com/o/r").as_deref(),
            Some("o/r")
        );
        assert_eq!(
            repo_flag("gh pr create --repo \"o/r\"").as_deref(),
            Some("o/r")
        );
        assert_eq!(repo_flag("gh pr create --title x"), None);
    }

    #[test]
    fn finds_writing_files() {
        for path in [
            "README.md",
            "docs/guide.md",
            "a/README",
            "CHANGELOG",
            "site/page.mdx",
            "package.json",
        ] {
            assert!(is_writing_file(path), "{path}");
        }
        for path in ["src/a.rs", "package-lock.json", "Cargo.toml"] {
            assert!(!is_writing_file(path), "{path}");
        }
    }

    #[test]
    fn finds_comment_lines() {
        for line in [
            "// why",
            "    /* note",
            " * more",
            " */",
            "# note",
            "<!-- x -->",
        ] {
            assert!(is_comment_line(line), "{line}");
        }
        for line in ["#[derive(Debug)]", "#include <a.h>", "let a = b * c;", "x"] {
            assert!(!is_comment_line(line), "{line}");
        }
    }

    #[test]
    fn reads_a_diff() {
        let code = "diff --git a/src/a.rs b/src/a.rs\n--- a/src/a.rs\n+++ b/src/a.rs\n@@ -1 +1 @@\n-let a = 1;\n+let a = 2;\n";
        assert!(!diff_has_writing(code));
        let comment = "--- a/src/a.rs\n+++ b/src/a.rs\n@@ -1 +1,2 @@\n+// why two\n+let a = 2;\n";
        assert!(diff_has_writing(comment));
        let removed = "--- a/src/a.rs\n+++ b/src/a.rs\n@@ -1,2 +1 @@\n-// old note\n let a = 2;\n";
        assert!(!diff_has_writing(removed));
        let readme = "--- a/README.md\n+++ b/README.md\n@@ -1 +1 @@\n-a\n+b\n";
        assert!(diff_has_writing(readme));
    }
}
