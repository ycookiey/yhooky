//! private-word-guard (PreToolUse, Bash).
//!
//! Before a `git push`, looks for words that may tell about the person in
//! what has not reached any remote yet (`HEAD --not --remotes`): the lines
//! the commits add and their messages. Private repositories count too, as
//! they may be made public later; repositories meant for personal use from
//! the start are listed to skip. The list lives outside every repository:
//! `$PRIVATE_WORDS_FILE`, else `~/.claude/private-words.toml`:
//!
//! ```toml
//! words = ["a name", "a place"]      # matched ignoring ASCII case
//! skip_repos = ["owner/repo"]        # GitHub owner/repo of origin, or its URL
//! ```
//!
//! The words are candidates to think over, not a ban, so the list can be
//! broad: a push that holds them is stopped once per session, repository
//! and set of places found; the same push sent again goes through. Which
//! were stopped is kept as empty files in `$PRIVATE_WORDS_STATE_DIR`, else
//! `<temp>/claude-private-words`. Without the list nothing is checked.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::Deserialize;

use super::post_push::runs_git_push;
use crate::docs;
use crate::input::Input;
use crate::output::{Finding, Kind};
use crate::public_repo::{git_config, github_repo, origin_url};
use crate::repo;
use crate::text::windows_path;

const ID: &str = "private-word-guard";
const LIMIT: usize = 20;

#[derive(Deserialize, Default)]
struct List {
    #[serde(default)]
    words: Vec<String>,
    #[serde(default)]
    skip_repos: Vec<String>,
}

fn list_file() -> Option<PathBuf> {
    match std::env::var("PRIVATE_WORDS_FILE") {
        Ok(file) if !file.is_empty() => Some(PathBuf::from(windows_path(&file))),
        _ => crate::home().map(|h| h.join(".claude").join("private-words.toml")),
    }
}

/// One place a word was found.
#[derive(Debug, PartialEq)]
pub struct Hit {
    pub word: String,
    pub commit: String,
    /// `path:line`, or "message".
    pub place: String,
}

/// Whether `word` occurs in `text`. An end of the word that is an ASCII
/// letter or digit must not run on into another one, so a short ASCII word
/// is not found inside a longer one (a user name inside an account name).
pub fn contains_word(text: &str, word: &str) -> bool {
    let alnum = |c: Option<char>| c.is_some_and(|c| c.is_ascii_alphanumeric());
    let (first, last) = (word.chars().next(), word.chars().last());
    text.match_indices(word).any(|(at, _)| {
        let before = text[..at].chars().next_back();
        let after = text[at + word.len()..].chars().next();
        let runs_on_before = alnum(first) && alnum(before);
        let runs_on_after = alnum(last) && alnum(after);
        !runs_on_before && !runs_on_after
    })
}

/// The words in `git log -p --format=COMMIT%x00%h%x00%B` output: in added
/// lines and in messages.
pub fn find(log: &str, words: &[String]) -> Vec<Hit> {
    let lowered: Vec<String> = words.iter().map(|w| w.to_lowercase()).collect();
    let mut hits = Vec::new();
    let mut commit = String::new();
    let mut in_message = false;
    let mut file = String::new();
    let mut line_no = 0usize;
    let look = |text: &str, commit: &str, place: String, hits: &mut Vec<Hit>| {
        let text = text.to_lowercase();
        for (word, low) in words.iter().zip(&lowered) {
            if !low.is_empty() && contains_word(&text, low) {
                hits.push(Hit {
                    word: word.clone(),
                    commit: commit.to_string(),
                    place: place.clone(),
                });
            }
        }
    };
    for line in log.lines() {
        if let Some(rest) = line.strip_prefix("COMMIT\0") {
            let (sha, first) = rest.split_once('\0').unwrap_or((rest, ""));
            commit = sha.to_string();
            in_message = true;
            look(first, &commit, "message".into(), &mut hits);
        } else if in_message && line.starts_with("diff --git ") {
            in_message = false;
        } else if in_message {
            look(line, &commit, "message".into(), &mut hits);
        } else if let Some(path) = line.strip_prefix("+++ ") {
            file = path.strip_prefix("b/").unwrap_or(path).to_string();
        } else if let Some(hunk) = line.strip_prefix("@@ ") {
            // "-a,b +c,d @@": the added lines count from c.
            line_no = hunk
                .split_whitespace()
                .find_map(|p| p.strip_prefix('+'))
                .and_then(|p| p.split(',').next())
                .and_then(|n| n.parse().ok())
                .unwrap_or(0);
        } else if let Some(added) = line.strip_prefix('+') {
            look(added, &commit, format!("{file}:{line_no}"), &mut hits);
            line_no += 1;
        } else if line.starts_with(' ') {
            line_no += 1;
        }
    }
    hits
}

/// The `origin` URL of the repository at `dir`, and its GitHub `owner/repo`
/// in lower case ("" elsewhere).
fn origin(dir: &Path) -> Option<(String, String)> {
    let url = git_config(dir)
        .and_then(|c| fs::read_to_string(c).ok())
        .and_then(|c| origin_url(&c))?;
    let name = github_repo(&url).unwrap_or_default().to_lowercase();
    Some((url, name))
}

pub fn run(input: &Input) -> Vec<Finding> {
    if input.event() != "PreToolUse" || input.tool() != "Bash" {
        return Vec::new();
    }
    let command = input.command();
    if !runs_git_push(&command) {
        return Vec::new();
    }
    let Some(list) = list_file()
        .and_then(|f| fs::read_to_string(f).ok())
        .and_then(|t| {
            toml::from_str::<List>(&t)
                .map_err(|e| crate::log(&format!("private-word-guard: {e}")))
                .ok()
        })
    else {
        return Vec::new();
    };
    if list.words.iter().all(|w| w.trim().is_empty()) {
        return Vec::new();
    }
    let dir = windows_path(&repo::dir(input, repo::First::GitC));
    let (url, name) = origin(Path::new(&dir)).unwrap_or_default();
    let listed = |s: &String| {
        (!url.is_empty() && s.eq_ignore_ascii_case(&url))
            || (!name.is_empty() && s.to_lowercase() == name)
    };
    if list.skip_repos.iter().any(listed) {
        return Vec::new();
    }
    // A private repository's own name is listed, yet is no secret inside it.
    let own = name.rsplit('/').next().unwrap_or_default();
    let words: Vec<String> = list
        .words
        .into_iter()
        .filter(|w| own.is_empty() || !w.eq_ignore_ascii_case(own))
        .collect();
    let Ok(out) = Command::new("git")
        .args([
            "-C",
            &dir,
            "log",
            "-p",
            "--no-color",
            "--no-ext-diff",
            "--format=COMMIT%x00%h%x00%B",
            "HEAD",
            "--not",
            "--remotes",
        ])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
    else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    let hits = find(&String::from_utf8_lossy(&out.stdout), &words);
    if hits.is_empty() {
        return Vec::new();
    }
    let all: Vec<String> = hits
        .iter()
        .map(|h| format!("「{}」 {} {}", h.word, h.commit, h.place))
        .collect();
    let state = crate::once::state_dir("PRIVATE_WORDS_STATE_DIR", "claude-private-words");
    let session = input.text("session_id");
    if !crate::once::first_time(&state, &format!("{session}\n{dir}\n{}", all.join("\n"))) {
        return Vec::new();
    }
    let more = all.len().saturating_sub(LIMIT);
    let rest = if more > 0 {
        format!(" ほか {more} 件")
    } else {
        String::new()
    };
    let see = docs::pointer("(", "git.md", " の「push 前に消す」)");
    vec![Finding::new(
        ID,
        Kind::Deny,
        format!(
            "push する commit に、個人を推測させうる語の候補(~/.claude/private-words.toml)がある: {}{rest}。\
             一度止めた。公開されうる repo に出してよいかを考え直す。出さないなら未 push の commit から消す\
             {see}。問題なければ、同じ push を出し直せば通る。",
            all[..all.len().min(LIMIT)].join("、")
        ),
    )]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_ascii_words_whole() {
        assert!(contains_word("path c:/users/ab/x", "ab"));
        assert!(!contains_word("github.com/abcd/repo", "ab"));
        assert!(!contains_word("concatenate", "cat"));
        assert!(contains_word("the cat漢字", "cat"));
        assert!(contains_word("a-b_c cat.", "cat"));
        assert!(contains_word("秘密語を含む", "秘密語"));
        assert!(contains_word("x秘密語y", "秘密語"));
        assert!(contains_word("my-repo-name here", "my-repo"));
        assert!(!contains_word("my-repository", "my-repo"));
    }

    #[test]
    fn finds_words_in_lines_and_messages() {
        let log = "COMMIT\0abc1234\0add the Secret page\n\nbody line\n\n\
                   diff --git a/x.md b/x.md\n--- a/x.md\n+++ b/x.md\n@@ -1,2 +1,3 @@\n keep\n+new SECRET here\n-old secret\n keep\n+plain\n\
                   COMMIT\0def5678\0other\n\ndiff --git a/y b/y\n+++ b/y\n@@ -0,0 +5 @@\n+また秘密語\n";
        let words = vec!["secret".to_string(), "秘密語".to_string()];
        let hits = find(log, &words);
        let places: Vec<(String, String, String)> = hits
            .into_iter()
            .map(|h| (h.word, h.commit, h.place))
            .collect();
        assert_eq!(
            places,
            vec![
                ("secret".into(), "abc1234".into(), "message".into()),
                ("secret".into(), "abc1234".into(), "x.md:2".into()),
                ("秘密語".into(), "def5678".into(), "y:5".into()),
            ]
        );
    }
}
