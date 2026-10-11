//! Which repository a command works on, and where it is pushed.

use std::process::Command;
use std::sync::LazyLock;

use regex::Regex;

use crate::input::Input;
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
}
