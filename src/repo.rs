//! Which repository a command works on, and where it is pushed.

use std::process::Command;
use std::sync::LazyLock;

use regex::Regex;

use crate::input::Input;
use crate::shell::{Dialect, Run};
use crate::text::{matches, strip_quote, windows_path};

static GIT_C: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"git[[:space:]]+-C[[:space:]]+[^[:space:]]+").unwrap());
static GIT_C_PREFIX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^git[[:space:]]+-C[[:space:]]+").unwrap());
static CD: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(^|[;&|[:space:]])cd[[:space:]]+[^;&|[:space:]]+").unwrap());
static CD_PREFIX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^.*cd[[:space:]]+").unwrap());

/// Where to look first for the folder a command works in.
#[derive(Clone, Copy)]
pub enum First {
    /// The last `git -C <dir>`, then the last `cd <dir>`.
    GitC,
    /// The last `cd <dir>`, then the last `git -C <dir>`.
    Cd,
}

fn last_git_c(command: &str) -> Option<String> {
    let last = matches(&GIT_C, command).pop()?;
    Some(GIT_C_PREFIX.replace(last, "").into_owned())
}

fn last_cd(command: &str) -> Option<String> {
    let last = matches(&CD, command).pop()?;
    Some(CD_PREFIX.replace(last, "").into_owned())
}

/// The folder `input`'s command works in: from the command, else the
/// hook's cwd. Quotes around it are taken off.
pub fn dir(input: &Input, first: First) -> String {
    let command = input.command();
    let found = match first {
        First::GitC => last_git_c(&command).or_else(|| last_cd(&command)),
        First::Cd => last_cd(&command).or_else(|| last_git_c(&command)),
    };
    let dir = found
        .filter(|d| !d.is_empty())
        .unwrap_or_else(|| input.cwd());
    strip_quote(strip_quote(&dir, '"'), '\'').to_string()
}

/// A change of folder, its options (`-P`, `-Path`) passed over.
static CD_HEAD: LazyLock<Regex> = LazyLock::new(|| {
    // cmd's `cd /d C:\x` changes the drive too.
    Regex::new(
        r#"(?i)^(?:cd|pushd|Set-Location)(?:\s+-\S+|\s+/d)*(?:\s+("[^"]*"|'[^']*'|[^\s;&|]+))?"#,
    )
    .unwrap()
});
/// `git -C <dir>` at the start of a part.
static GIT_C_ARG: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"^git(?:\s+-[^C\s]\S*)*\s+-C\s+("[^"]*"|'[^']*'|\S+)"#).unwrap());

/// `to` from the folder `from`: as it is when absolute (`/c/x`, `C:/x`,
/// `\\server\x`), from the home folder when it starts with `~`, else
/// joined to `from`. A folder named by a variable or a substitution
/// (`$dir`, `$(…)`) or filled in later (`xargs -I{} sh -c 'cd {}'`)
/// cannot be known here, so `from` is kept, and so is it past a length no
/// folder has (`cd a` many times), so that following stays in proportion.
fn change_dir(from: &str, to: &str) -> String {
    const LONGEST: usize = 4096;
    if from.len() + to.len() > LONGEST {
        return from.to_string();
    }
    let to = strip_quote(strip_quote(to, '"'), '\'');
    if to.contains('$') || to.contains("{}") {
        return from.to_string();
    }
    let bytes = to.as_bytes();
    if to.starts_with('/')
        || to.starts_with('\\')
        || (bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':')
    {
        return to.to_string();
    }
    if let Some(rest) = to.strip_prefix('~')
        && (rest.is_empty() || rest.starts_with('/') || rest.starts_with('\\'))
        && let Some(home) = crate::home()
    {
        return format!("{}{rest}", home.to_string_lossy());
    }
    format!("{}/{to}", from.trim_end_matches(['/', '\\']))
}

/// The folder the first command of `command` (Bash) that `is_target`
/// accepts (`git push`, `gh pr create`) runs in; see [`work_dir_in`].
pub fn work_dir(command: &str, cwd: &str, is_target: impl Fn(&str) -> bool) -> String {
    work_dir_in(command, cwd, Dialect::Bash, is_target)
}

/// The folder the first command of `command` that `is_target` accepts runs
/// in. Starting from `cwd`, the `cd`s run before it that reach it (in its
/// list, or one holding it: a `cd` in a subshell or `$(…)` stays there)
/// are followed in order, a relative one from the folder reached so far,
/// then a `git -C <dir>` at the start of the command itself. The commands
/// are those src/shell.rs reads, so a `cd` written in a PR body or a
/// commit message does not count. When none is accepted (the target is
/// found only by the plain match, [`crate::shell::as_text_finds`]), the
/// `cd`s before it in the parts [`crate::shell::as_text_parts`] gives,
/// outside heredoc bodies.
pub fn work_dir_in(
    command: &str,
    cwd: &str,
    dialect: Dialect,
    is_target: impl Fn(&str) -> bool,
) -> String {
    let runs = crate::shell::runs_of(command, dialect);
    let follow = |until: usize, reaches: &dyn Fn(&Run) -> bool| {
        let mut dir = cwd.to_string();
        for run in &runs[..until] {
            if reaches(run)
                && let Some(c) = CD_HEAD.captures(&run.text)
                && let Some(to) = c.get(1)
                && to.as_str() != "-"
            {
                dir = change_dir(&dir, to.as_str());
            }
        }
        dir
    };
    match runs.iter().position(|run| is_target(&run.text)) {
        Some(at) => {
            let target = &runs[at];
            let mut dir = follow(at, &|run| run.reaches(target));
            if let Some(c) = GIT_C_ARG.captures(&target.text) {
                dir = change_dir(&dir, &c[1]);
            }
            dir
        }
        // Found only by the plain match: its folder from the plainly split
        // parts outside heredoc bodies, up to the target or to the end.
        None => {
            let kept = match dialect {
                Dialect::Bash => crate::shell::split_heredocs(command).0,
                Dialect::PowerShell => command.to_string(),
            };
            let mut dir = cwd.to_string();
            for head in crate::shell::as_text_parts(&kept) {
                if crate::shell::as_text_finds(head, &is_target) {
                    if let Some(c) = GIT_C_ARG.captures(head) {
                        dir = change_dir(&dir, &c[1]);
                    }
                    return dir;
                }
                if let Some(c) = CD_HEAD.captures(head)
                    && let Some(to) = c.get(1)
                    && to.as_str() != "-"
                {
                    dir = change_dir(&dir, to.as_str());
                }
            }
            dir
        }
    }
}

/// Every command `command` (Bash) runs but a `cd`, as `(folder, command)`,
/// read by src/shell.rs (text in quotes or heredoc bodies is not a command;
/// what a shell is handed is): the folder follows the `cd`s run before it
/// that reach it, from `cwd`, as in [`work_dir_in`], then a `git -C <dir>`
/// at the start of the command itself.
pub fn parts_with_dirs(command: &str, cwd: &str) -> Vec<(String, String)> {
    let runs = crate::shell::runs_of(command, Dialect::Bash);
    // The folder each open list has reached, outermost first: a list the
    // reading has left is not entered again, so it is dropped.
    let mut reached: Vec<(&Run, String)> = Vec::new();
    let mut parts = Vec::new();
    for run in &runs {
        while reached
            .last()
            .is_some_and(|(cd, _)| !cd.reaches_scope(run.scope()))
        {
            reached.pop();
        }
        let dir = reached.last().map_or(cwd, |(_, dir)| dir.as_str());
        if let Some(c) = CD_HEAD.captures(&run.text) {
            if let Some(to) = c.get(1)
                && to.as_str() != "-"
            {
                let moved = change_dir(dir, to.as_str());
                reached.push((run, moved));
            }
            continue;
        }
        let dir = match GIT_C_ARG.captures(&run.text) {
            Some(c) => change_dir(dir, &c[1]),
            None => dir.to_string(),
        };
        parts.push((dir, run.text.clone()));
    }
    parts
}

/// `path` as seen from the folder `dir` (see `change_dir`).
pub fn resolve(dir: &str, path: &str) -> String {
    change_dir(dir, path)
}

/// The URL of `dir`'s `origin`, or "" when there is none.
pub fn origin(dir: &str) -> String {
    Command::new("git")
        .args(["-C", &windows_path(dir), "remote", "get-url", "origin"])
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(command: &str) -> Input {
        let raw = serde_json::json!({ "cwd": "C:/here", "tool_input": { "command": command } });
        Input::parse(&raw.to_string()).unwrap()
    }

    #[test]
    fn finds_the_folder() {
        assert_eq!(
            dir(&input("cd /c/a && git -C /c/b push"), First::GitC),
            "/c/b"
        );
        assert_eq!(
            dir(&input("cd /c/a && git -C /c/b push"), First::Cd),
            "/c/a"
        );
        assert_eq!(dir(&input("cd \"/c/x y\" && ls"), First::Cd), "/c/x");
        assert_eq!(dir(&input("git push"), First::Cd), "C:/here");
        assert_eq!(dir(&input("cd '/c/q' && git push"), First::GitC), "/c/q");
    }

    #[test]
    fn follows_the_cds_before_the_target() {
        let push = |head: &str| head.starts_with("git push") || head.contains(" push");
        let at = |command: &str| work_dir(command, "C:/here", push);
        assert_eq!(at("git push"), "C:/here");
        assert_eq!(at("cd /c/a && git push"), "/c/a");
        assert_eq!(at("cd /c/a && cd sub && git push"), "/c/a/sub");
        assert_eq!(at("cd sub; git push"), "C:/here/sub");
        assert_eq!(at("cd \"C:/x y\" && git push"), "C:/x y");
        assert_eq!(at("cd /c/a && git -C ../b push"), "/c/a/../b");
        assert_eq!(at("git -C \"/c/q r\" push"), "/c/q r");
        assert_eq!(
            at("git push && cd /c/later"),
            "C:/here",
            "cd after the push"
        );
        assert_eq!(
            at(
                "r=$(git rev-parse --abbrev-ref HEAD); cd /c/wt && git push -u origin \"$r\" \
                && gh pr create --body \"$(cat <<'EOF'\ncd startpage && npm i\nEOF\n)\""
            ),
            "/c/wt",
            "cd in a heredoc body"
        );
        assert_eq!(
            at("cd /c/wt && git push && gh pr create --body 'run: cd x'"),
            "/c/wt",
            "cd in quotes"
        );
    }

    #[test]
    fn finds_where_pushes_and_gh_run() {
        use crate::builtin::public_writing::gh_dir;
        use crate::builtin::push_check::push_dir;
        let here = "C:/here";
        for (command, want) in [
            ("cd /c/a && out=$(git push 2>&1)", "/c/a"),
            ("cd /c/a && bash <<'EOF'\ngit push\nEOF", "/c/a"),
            ("cd /c/a && git -C \"$d\" push", "/c/a"),
            ("cd /c/a && git -C sub push", "/c/a/sub"),
            ("Set-Location -Path C:/a; git push", "C:/a"),
            ("cd -P /c/a && git push", "/c/a"),
            ("cd /c/a && cd - && git push", "/c/a"),
        ] {
            assert_eq!(push_dir(command, here), want, "{command}");
        }
        for (command, want) in [
            (
                "cd /c/a && gh pr create --body \"see git -C sub push\"",
                "/c/a",
            ),
            ("cd /c/a && url=$(gh pr create --fill)", "/c/a"),
            ("cd /c/a && git push && gh release create v1", "/c/a"),
        ] {
            assert_eq!(gh_dir(command, here, Dialect::Bash), want, "{command}");
        }
    }

    #[test]
    fn gives_each_command_its_folder() {
        let parts = parts_with_dirs(
            "(cd /c/a && git add x); git add y; git -C /c/b add z; cat > m.md <<'EOF'\ngit add w\nEOF",
            "C:/here",
        );
        let of = |command: &str| {
            parts
                .iter()
                .find(|(_, c)| c == command)
                .map(|(d, _)| d.as_str())
        };
        assert_eq!(of("git add x"), Some("/c/a"));
        assert_eq!(of("git add y"), Some("C:/here"));
        assert_eq!(of("git -C /c/b add z"), Some("/c/b"));
        assert_eq!(of("git add w"), None, "a heredoc body is not a command");
    }
}
