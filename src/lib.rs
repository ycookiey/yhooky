//! One binary for Claude Code's hooks. It reads the hook's input once, runs
//! every rule for the event in this one process (rules written as data in
//! TOML files, and built-in checks), and prints one answer.
//!
//! The rules come from `~/.claude/hooks/rules` and, in a project, the
//! `.claude/hooks/rules` of `$CLAUDE_PROJECT_DIR` or of the nearest folder
//! above it, within its git repository, that has one.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub mod builtin;
pub mod cases;
pub mod docs;
pub mod explorer;
pub mod input;
pub mod once;
pub mod output;
pub mod public_repo;
pub mod repo;
pub mod rules;
pub mod shell;
pub mod text;

use input::Input;
use output::Finding;
use rules::Rules;

/// Like `println!`, but ignores write errors instead of panicking: a reader
/// that stops early (`yhooky notices | head`) closes the pipe, and that is
/// not an error to report. Other write errors are ignored too.
#[macro_export]
macro_rules! outln {
    ($($arg:tt)*) => {{
        use std::io::Write as _;
        let _ = writeln!(std::io::stdout().lock(), $($arg)*);
    }};
}

/// The user's home: HOME (as Git Bash has it, or a Windows path), else
/// USERPROFILE.
pub fn home() -> Option<PathBuf> {
    std::env::var("HOME")
        .ok()
        .filter(|h| !h.is_empty())
        .map(|h| PathBuf::from(text::windows_path(&h)))
        .or_else(|| std::env::var_os("USERPROFILE").map(PathBuf::from))
}

fn rules_in(dir: &Path) -> PathBuf {
    dir.join(".claude").join("hooks").join("rules")
}

/// The folder whose `.claude/hooks/rules` holds the project's rules: the
/// nearest of `start` and the folders above it that has one, looking no
/// higher than the top of the git repository `start` is in (the folder with
/// `.git`, a folder or, in a worktree or submodule, a file). A session
/// started in a subfolder of a repository thus gets the repository's rules,
/// and a repository inside another one does not get the outer one's.
pub fn find_project(start: &Path) -> Option<PathBuf> {
    for dir in start.ancestors() {
        if rules_in(dir).is_dir() {
            return Some(dir.to_path_buf());
        }
        if dir.join(".git").exists() {
            return None;
        }
    }
    None
}

/// The project folder for `$CLAUDE_PROJECT_DIR` (see `find_project`).
pub fn project_dir() -> Option<PathBuf> {
    let start = std::env::var("CLAUDE_PROJECT_DIR").ok()?;
    find_project(&PathBuf::from(text::windows_path(&start)))
}

/// The folders rules are read from, in order: the user's, then the
/// project's (see `find_project`) when it is another folder.
pub fn rule_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(home) = home() {
        dirs.push(rules_in(&home));
    }
    if let Some(project) = project_dir() {
        let dir = rules_in(&project);
        let same = dirs.first().is_some_and(|d| {
            d.canonicalize()
                .ok()
                .zip(dir.canonicalize().ok())
                .is_some_and(|(a, b)| a == b)
        });
        if !same {
            dirs.push(dir);
        }
    }
    dirs
}

/// Appends a line to ~/.claude/logs/yhooky.log. Never fails the hook.
pub fn log(line: &str) {
    let Some(dir) = home().map(|h| h.join(".claude").join("logs")) else {
        return;
    };
    let _ = std::fs::create_dir_all(&dir);
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    if let Ok(mut file) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("yhooky.log"))
    {
        let _ = writeln!(file, "{secs} {line}");
    }
}

/// Everything the rules find for `input`: the data rules first, then the
/// built-in checks the rule files turn on, each in its order.
pub fn findings(input: &Input, rules: &Rules) -> Vec<Finding> {
    let mut found = rules.run(input);
    for name in &rules.builtins {
        match builtin::find(name) {
            Some(check) => found.extend(check(input)),
            None => log(&format!("unknown builtin {name}")),
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn finds_the_rules_of_a_folder_above_within_the_repository() {
        let root = tempfile::tempdir().unwrap();
        let repo = root.path().join("repo");
        let sub = repo.join("a").join("b");
        fs::create_dir_all(rules_in(&repo)).unwrap();
        fs::create_dir_all(repo.join(".git")).unwrap();
        fs::create_dir_all(&sub).unwrap();
        assert_eq!(find_project(&repo), Some(repo.clone()));
        assert_eq!(find_project(&sub), Some(repo.clone()));
    }

    #[test]
    fn prefers_the_nearest_folder_with_rules() {
        let root = tempfile::tempdir().unwrap();
        let repo = root.path().join("repo");
        let sub = repo.join("sub");
        fs::create_dir_all(rules_in(&repo)).unwrap();
        fs::create_dir_all(rules_in(&sub)).unwrap();
        fs::create_dir_all(repo.join(".git")).unwrap();
        assert_eq!(find_project(&sub.join("x")), Some(sub.clone()));
    }

    #[test]
    fn stops_at_the_top_of_an_inner_repository() {
        let root = tempfile::tempdir().unwrap();
        let outer = root.path().join("outer");
        let inner = outer.join("inner");
        fs::create_dir_all(rules_in(&outer)).unwrap();
        fs::create_dir_all(outer.join(".git")).unwrap();
        fs::create_dir_all(inner.join("src")).unwrap();
        // A worktree or submodule has a `.git` file, not a folder.
        fs::write(inner.join(".git"), "gitdir: elsewhere").unwrap();
        assert_eq!(find_project(&inner.join("src")), None);
    }
}
