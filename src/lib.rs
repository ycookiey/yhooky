//! One binary for Claude Code's hooks. It reads the hook's input once, runs
//! every rule for the event in this one process (rules written as data in
//! TOML files, and built-in checks), and prints one answer.
//!
//! The rules come from `~/.claude/hooks/rules` and, in a project,
//! `$CLAUDE_PROJECT_DIR/.claude/hooks/rules`.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

pub mod builtin;
pub mod cases;
pub mod explorer;
pub mod input;
pub mod once;
pub mod output;
pub mod public_repo;
pub mod repo;
pub mod rules;
pub mod text;

use input::Input;
use output::Finding;
use rules::Rules;

/// The user's home: HOME (as Git Bash has it, or a Windows path), else
/// USERPROFILE.
pub fn home() -> Option<PathBuf> {
    std::env::var("HOME")
        .ok()
        .filter(|h| !h.is_empty())
        .map(|h| PathBuf::from(text::windows_path(&h)))
        .or_else(|| std::env::var_os("USERPROFILE").map(PathBuf::from))
}

/// The folders rules are read from, in order: the user's, then the
/// project's when it is another folder.
pub fn rule_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(home) = home() {
        dirs.push(home.join(".claude").join("hooks").join("rules"));
    }
    if let Ok(project) = std::env::var("CLAUDE_PROJECT_DIR") {
        let dir = PathBuf::from(text::windows_path(&project))
            .join(".claude")
            .join("hooks")
            .join("rules");
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
